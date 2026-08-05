# Letta compared to our session event catalog

Part of Session Store Research.
Produced by running [RESEARCH_PROMPT_COMPARISON](../../RESEARCH_PROMPT_COMPARISON.md).
Stage-one dossier: [Letta](./index.md).
Compared against `proto/trogonai/session/sessions/v1alpha1/` and [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) on 2026-08-04.

**Store maturity: 11/12** -- evolution scars 3/3 (167 Alembic migration files with a
real cutover in progress: `alembic/versions/e991d2e3b428_add_monotonically_increasing_ids_to_.py:1-40`
added `messages.sequence_id` and backfilled existing rows ordered by
`["created_at", "id"]`, and `alembic/versions/27de0f58e076_add_conversations_tables_and_run_.py:1-45`
added the entire `conversations`/`conversation_messages` relational model
mid-migration away from the legacy `message_ids` JSON array), operational age
2/3 (real production hardening -- a deadlock-retry decorator wrapping nearly
every ORM write, `db_registry.async_session()` retrying transient
`ConnectionError`s up to three times with backoff at `letta/server/db.py:84-116`,
and an application-level self-healing pass, `backfill_missing_tool_call_ids`
(`letta/services/message_manager.py:30-113`), explicitly tied to "historical
messages (oct 1-6, 2025 bug)" at `message_manager.py:113` -- but none of this is
corroborated by an external issue tracker the way Cline's `cline#9011` is, so
it is scored short of 3), exposure 3/3 (a funded, shipped, multi-tenant server:
`OrganizationMixin` (`letta/orm/mixins.py:1-99`) scopes every row to a tenant,
and `letta/server/db.py:24-49` configures asyncpg specifically for
PgBouncer/transaction-pooling compatibility -- disabled statement caching, a
UUID-suffixed prepared-statement name per connection -- which is production
multi-host deployment evidence, not a toy default), design independence 3/3
(no pluggable storage adapter and no evidence of persistence code inherited
from a fork; the store is Letta's own SQLAlchemy ORM plus service-layer
managers, evolved from its own predecessor MemGPT, not copied from another
product).

## The one structural difference everything else follows from

Letta durably separates two things: a mostly-append `messages` row set
(`letta/orm/message.py:1-266`), and a thin, mutable, unguarded **pointer**
that says which subset of that row set is currently "in context" for the LLM.
In the legacy model that pointer is `Agent.message_ids`, a JSON array column
directly on the agent row (`letta/orm/agent.py:71`), and Letta's own source
calls it out as a known anti-pattern immediately above the field:

```python
# letta/orm/agent.py:69-70
# TODO: This should be a separate mapping table
# This is dangerously flexible with the JSON type
```

`AgentManager.reset_messages_async`'s own docstring states the consequence
plainly: "Note: This only clears messages from the agent's context, it does
not delete them from the database" (`letta/services/agent_manager.py:1686`).
The newer relational model replaces the JSON array with a per-row
`ConversationMessage.in_context` boolean (`letta/orm/conversation_messages.py`),
but it is the same shape of problem: state that determines what the model
sees next turn, that cannot be reconstructed by replaying the message log,
and that is mutated in place. Concurrency control for this pointer does not
exist -- `Agent.message_ids` updates are plain last-write-wins full-column
overwrites (`letta/services/agent_manager.py:1713`) with no version check, so
two concurrent turns racing to update the same agent's context pointer can
silently clobber each other's view of "what's in context," even though the
underlying `messages` rows are never lost.

We have no analogue of this pointer, anywhere. [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 8 states
that "the model-visible context is compiled deterministically from the event
log bounded by the latest `Compacted` marker" -- it is a fold, recomputed on
demand, never a separately-mutated column that a writer can race against or
forget to update correctly. Where Letta needs a second concurrency story for
its pointer, and does not reliably have one (optimistic-concurrency version
checking, SQLAlchemy's `version_id_col`, is used on exactly one model in the
whole ORM layer, `Block` -- `letta/orm/block.py:61` -- confirmed by the
dossier's own grep of every file under `letta/orm/` for `version_id_col`,
which returns only that one hit), we need none, because there is no second
piece of authoritative state to protect. The fold *is* the pointer, derived
each time from facts already governed by our own `WRITE_PRECONDITION`
classification ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 2). Letta is unusually strong evidence for
this design choice specifically because it is a funded, shipped product whose
own maintainers flagged the risk in a code comment and are mid-migration away
from it -- not a hypothetical failure mode, a self-documented one.

## Mapping

Two words collide across the two designs and deserve naming before the
table, because a naive lookup on either would silently produce a wrong
mapping (per the Method's warning on semantic mismatches):

- **"Session."** The dossier is explicit that Letta has no first-class
  Session object at all: the durable, long-lived entity is the `Agent` row
  itself (`letta/orm/agent.py:1-524`), created once and persisting
  indefinitely, with no separate record that expires or gets swapped out.
  "Resuming a session" in Letta means "sending another message to the same
  `agent_id`." Our `session_id` names a bounded execution
  (`SessionStarted`) that reaches a terminal state (`SessionClosed`,
  `SessionCancelled`, `SessionFailed`, `SessionHidden`) and is never resumed
  as the same identity again -- continuation is a new `SessionForked` stream.
  Nothing in our catalog corresponds to an identity that simply never
  terminates.
- **"Checkpoint."** `letta/services/block_manager_git.py` calls its
  git-backed memory-block versioning system a "checkpoint" concept in the
  dossier's own section heading, and once git-memory is enabled for an
  agent, `Block.value` in Postgres becomes a read cache and the git
  repository becomes authoritative -- `sync_blocks_from_git`'s docstring says
  so directly: "rebuild the PostgreSQL cache from git source of truth"
  (`letta/services/block_manager_git.py:571`). Our `Checkpoint`
  (`checkpoint.proto`) is never a second source of truth: [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 3
  is explicit that the typed event log is always authoritative and a harness
  recovery checkpoint is "an opaque artifact used only when the platform
  continues process state from an in-flight harness loop," discarded on
  corruption in favor of replay. Letta's git-checkpoint inverts that
  relationship for one tier of its own data (core memory): the durable SQL
  store becomes the disposable cache and an external system becomes
  authoritative. This is the sharpest semantic mismatch in the comparison --
  see the "What our design already does better" section.

| Letta | Ours | Verdict |
| --- | --- | --- |
| `Agent.id` (long-lived, never terminates, `letta/orm/agent.py:1-524`) | `SessionId` (bounded, terminal-lifecycle) | Semantic mismatch -- see above; no equivalent to a persistent identity that outlives a bounded execution |
| `Conversation.id` (secondary, concurrent-messaging scope within one agent, `letta/orm/conversation.py`) | No equivalent -- a Session has exactly one linear turn sequence | Gap, by design: our Non-Goals defer a symmetric multi-party `Conversation` aggregate |
| `Run.id` (execution-attempt record per processing turn, optional `conversation_id` FK, `letta/orm/run.py:22-57`, docstring at `letta/orm/run.py:23-25`) | `ExecutionAttemptStarted`/`Ready`/`Ended` (`execution_attempt_started.proto`) | Equivalent -- both are "one attempt" units distinct from the parent identity |
| `Message.id` / `sequence_id` (`BigInteger`, unique, monotonic order key, `letta/orm/message.py`) | `CanonicalMessage.message_id` (`message.proto`); order is `SessionOrdinal`, fold-derived, never a stored counter (`session_ordinal.proto`) | Trade-off -- see below |
| `Agent.message_ids` JSON array / `ConversationMessage.in_context` (mutable, unguarded pointer) | No equivalent; model-visible context folds from the newest `Compacted` marker ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 8) | Ours, decisively -- the structural difference above |
| `otid` ("offline threading ID," a schema field on the message payload itself, `letta/schemas/message.py`, the documented dedup key for retried sends) | The command idempotency key ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 2), which "lives on the command, not the event" (decision 3) | Ours, decisively -- Letta conflates dedup identity with domain data; we deliberately never let a domain payload carry its own dedup key |
| `Block.version_id_col` (optimistic concurrency, the *only* model with it, `letta/orm/block.py:61`) | `WRITE_PRECONDITION` classified per fact (`NoStream`/`At`/`Any`, [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 2), applied to every invariant-bearing transition, not just one entity | Ours, decisively -- see "What not to copy" |
| `BlockHistory` (always-on undo/redo snapshot table, full-value copy per change, cascade-deleted with its `Block`, `letta/orm/block_history.py:12-49`) | No equivalent entity to snapshot; the event log itself is the undo/redo trail, kept forever ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 7) | Ours -- no second table can drift from the log it is supposed to mirror, because there is nothing but the log |
| Git-backed memory-block checkpoints, opt-in, Postgres becomes a cache (`letta/services/block_manager_git.py`, `letta/services/memory_repo/git_operations.py`) | `Checkpoint`/`CheckpointProduced` (`checkpoint.proto`, `checkpoint_produced.proto`), always disposable, log always authoritative (decision 3) | Semantic mismatch -- see above |
| `ConversationManager.fork_conversation`: genuine shared-prefix fork, links the *same* `Message` rows into a new `Conversation` via new junction rows, not a copy (`letta/services/conversation_manager.py:105-174`) | `SessionForked{source_session_id, context_prefix_boundary}` (`session_forked.proto`); inherited by reference through a context projection keyed by `(source_session_id, context_prefix_boundary)`, never a physical copy ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 5) | Ours, validated independently -- two unrelated designs converged on "fork is a reference, not a copy" |
| `delete_conversation`'s reference-counted soft-delete: only removes a `Message` row if no other live conversation still references it via an explicit `NOT IN` subquery against `conversation_messages` (`letta/services/conversation_manager.py:582-597`) | No equivalent bookkeeping needed -- a fork never takes ownership of a source event, it only references it, so there is no reference count to maintain when a fork is retired | Ours, decisively -- the reference-counting problem does not arise because facet 5 never lets an event be co-owned in the first place |
| `Group`/`groups_agents`, `Group.manager_agent_id` FK `ondelete="RESTRICT"` (`letta/orm/group.py:1-43,24`); sleeptime agents are sibling `Agent` rows, not entries nested in a parent transcript | `DelegationDispatched`/`ParentLinked` (`delegation_dispatched.proto`, `parent_linked.proto`); a child session is its own logical stream ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 6) | Similar shape (sibling stream, not nested), different lifecycle -- see "Subagent cascade" below |
| `archives_agents` junction, `ondelete="CASCADE"` on both directions but only removes the *attachment* row (`letta/orm/archives_agents.py:23-24`); no code path calls the archive's own hard delete from agent deletion | `ArtifactErased` (`artifact_erased.proto`) is a deliberate, separately-invoked event; nothing is ever *implicitly* orphaned because artifacts are never implicitly multi-owned the way an archive can be | Ours, decisively -- see "What not to copy" |
| No scheduled retention/TTL job found (`letta/jobs/scheduler.py` grepped for `TTL`/`expire`/`retention`/`cleanup`, only an unrelated log line at `scheduler.py:228`) | `SessionHidden` (visibility tombstone), `RedactionApplied` (read-time mask), `ArtifactErased` (byte destruction) -- an explicit three-tier privacy contract ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 7) | Ours, decisively -- Letta has no policy at all, ours has a stated one even though both keep bytes |
| Turbopuffer vector index: fire-and-forget background embedding, no backfill/reindex code path found anywhere (`letta/helpers/tpuf_client.py`, `letta/services/archive_manager.py`, `letta/services/message_manager.py` all grepped) | No full-text/vector-search subsystem defined yet; [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 8 explicitly scopes any future one as "a separate, independently bootstrapped projection off the same log, out of scope here" | Open risk on our side too -- see recommendation 3 |
| `list_messages`, cursor-paginated on `sequence_id` with `after`/`before` semantics, not offset (`letta/services/message_manager.py:1001-1024`) | `list_sessions`/`get_session` over a fold-derived KV projection, checkpointed by `last_applied_stream_position` ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 8) | Equivalent design principle: resume/list cost tracks a cursor, not table size |

## What we should consider changing

Ordered by how consequential the underlying question is, not by
implementation cost.

### 1. Decide explicitly whether a persistent, cross-Session "agent identity" belongs in the ADR chain, or whether fork is meant to be the only continuity mechanism

**The change.** Add an explicit answer -- in [ADR#0031](../../../../adr/0031-agent-implementation-and-session-plan.md) or [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) -- to
whether our platform needs a stable identity that spans many bounded
Sessions the way Letta's `Agent.id` spans arbitrarily many `Conversation`s
and `Run`s, or whether "fork a new Session from the last one"
(`SessionForked.source_session_id`, decision 5) is intended to be the sole
continuity primitive a caller ever needs.

**Evidence anchor.** Letta, store maturity 11/12: `Agent.id` is the one
addressable identity every client operation takes (all message send/read
operations take an `agent_id`, `letta/orm/agent.py:1-524`); it never expires
and is never re-created. "Session," "conversation," and "resuming" are all
relative to that one persistent id. Our closest analogue,
`SessionForked{source_session_id, context_prefix_boundary}`
(`session_forked.proto`), mints a wholly new `session_id` for every
continuation, with only a one-hop backward pointer to its immediate
predecessor -- reconstructing "every Session that is really the same ongoing
assistant relationship" requires walking a fork chain of unknown length
rather than a single indexed lookup on a stable id.

**Blast radius.** Additive, if adopted: a new optional identifier (for
example `agent_continuity_id`) threaded through `SessionStarted` and
`SessionForked`, populated by the caller, with a new projection indexed on
it. It would not change `session_id`, `SessionForked.source_session_id`, or
the fork-by-reference mechanism itself.

**Why it is a good idea, or why it is not.** This is recorded as an open
question, not a firm recommendation, because [ADR#0031](../../../../adr/0031-agent-implementation-and-session-plan.md) already scopes a
Session to "one execution of a pinned agent revision" and [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md)'s
Non-Goals explicitly defer "mid-session model or runner switching" -- which
reads as a deliberate choice that a *revision change* forces a new Session
identity, with continuity expressed only through forking. That may already
be the intended, sufficient answer; Letta's opposite choice (one identity,
never terminates, arbitrarily many sub-scopes) is the strongest evidence in
the corpus that a real, shipped, funded product chose the other design, so
the question is worth settling explicitly rather than leaving it to be
inferred from what forking happens to make possible.

**What it costs us.** If adopted: a new field on two creation events, a new
projection to maintain, and a product decision about whether the id is
caller-supplied or platform-minted. If rejected: nothing, but the rejection
itself should be recorded so this is not re-proposed on the strength of
Letta's example alone.

### 2. State explicitly, as a standing rule, that no future field on the model-visible-context projection may become an authoritative, unguarded, last-write-wins pointer

**The change.** Add a sentence to [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 8 (or a Non-Goal)
stating that the model-visible-context compilation must remain a pure fold
of the event log, and that no future optimization (for example, caching
"the current in-context message set" as a denormalized, directly-writable
field for fast lookup) may be introduced without the same OCC discipline
decision 2 already applies to every other invariant-bearing transition.

**Evidence anchor.** Letta, store maturity 11/12: `Agent.message_ids`
(`letta/orm/agent.py:71`) began, by the source's own comment, as exactly this
kind of convenience -- a directly-mutable field standing in for what should
be derived -- and is now flagged in the same file as a known anti-pattern
the team is migrating away from (`letta/orm/agent.py:69-70`), with no
concurrency guard at all on its updates (`letta/services/agent_manager.py:1713`).

**Blast radius.** Additive -- a documentation clarification against a design
that already avoids the pattern structurally (see "The one structural
difference" above); nothing in the current schema needs to change.

**Why.** The risk is not that today's design has this problem; it is that
a future implementer, chasing a legitimate performance concern (recomputing
the fold on every read is not free), could reintroduce a stored, mutable
"current window" field with good intentions and no guard, exactly as
Letta's own team apparently did. Naming the failure mode in the ADR, with
Letta's self-documented example as the citation, is cheap insurance against
a real, evidenced failure mode recurring in a design that otherwise avoids
it by construction.

**What it costs us.** Nothing beyond the sentence; it becomes real cost
only if a future implementer would otherwise have shipped the pattern.

### 3. Require a stated backfill/reindex contract before any full-text or vector-search projection ships

**The change.** [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 8 currently scopes any future full-text or
vector-search subsystem as "a separate, independently bootstrapped
projection off the same log, out of scope here." Before such a projection
ships, it should carry an explicit answer to "what happens to history that
predates the projection's existence, or predates a later policy change that
turns it on for previously-unindexed data."

**Evidence anchor.** Letta, store maturity 11/12: Turbopuffer is a
per-archive, settings-gated decision made once, at creation time, and cached
permanently (`letta/services/archive_manager.py:30-58`); the dossier's own
grep of `archive_manager.py`, `message_manager.py`, and `tpuf_client.py` for
`backfill`/`reindex`/`re-index` found no job that retroactively embeds
pre-existing rows, only a still-open TODO acknowledging a schema-field gap
in the opposite direction (`letta/helpers/tpuf_client.py:1053`). The
dossier's own inference: enabling Turbopuffer for the first time on a server
with existing history, or changing one archive's `vector_db_provider` after
the fact, means vector search silently never surfaces the pre-existing rows,
with no reconciliation job to catch it.

**Blast radius.** Additive -- this is a requirement on future work (any
search/vector projection built off the log), not a change to today's
schema or any shipped behavior.

**Why.** Our own model-visible-context fold and read-side projections
(decision 8) are already immune to this class of gap, because they are
recomputed from the full log on catch-up, not populated once at creation
time and left to drift. A future search projection is the one place that
same discipline could quietly lapse if nobody states the requirement up
front, because a naive implementation (index going forward only) is the
easy path and Letta shows exactly what it costs: a permanent, silent blind
spot with no error signal.

**What it costs us.** Nothing today; a real backfill job to design and
operate whenever such a projection is actually built.

### 4. Do not relax optimistic concurrency on any invariant-bearing (`At`) transition for the sake of write-path performance

**The change under consideration, and why to reject it.** A future proposal
to move one of the `At`-guarded events (for example `Compacted`,
`ExecutionAttemptStarted`, or `DelegationDispatched`) to `Any` in the name of
reducing head-check/retry overhead on a hot path.

**Evidence anchor.** Letta, store maturity 11/12: optimistic-concurrency
version checking exists for exactly one model in the entire ORM layer,
`Block` (`letta/orm/block.py:61`) -- memory-configuration data, not the
highest-churn, highest-consequence mutable state in the system. The actual
per-turn hot pointer, `Agent.message_ids`, has no guard at all
(`letta/services/agent_manager.py:1713`). Letta protected the tier least
likely to matter under real concurrent load and left the tier most likely to
matter completely exposed.

**Blast radius.** Breaking the decision -- [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 2's
`WRITE_PRECONDITION` classification table names exactly which facts need
`At` because `decide` genuinely branches on the current head for them
(one active attempt, mutually exclusive approve/deny, one terminal outcome
per ledger operation); moving any of those to `Any` for performance
contradicts that classification's stated invariant, not merely its wire
shape.

**Why not to do this.** Letta is direct cautionary evidence for the
opposite failure mode from the one decision 2 already guards against
(uniform OCC, rejected in Alternatives as taxing the high-volume path for no
gain): guarding the *wrong* tier, or guarding it inconsistently, is at least
as dangerous as guarding nothing, because it creates false confidence that
"we have OCC" when the actually load-bearing state has none. This
recommendation exists to make that failure mode explicit before it is
proposed as a performance optimization, since a `WRITE_PRECONDITION`
softened "just for this one hot path" is exactly how Letta's asymmetry
arose in the first place -- one entity at a time, each individually
reasonable.

**What it costs us.** Nothing to reject it; the cost this recommendation
guards against is the one a future relaxation would introduce.

## What our design already does better

- **No mutable, unguarded pointer stands between the log and the model's
  view.** Letta's `Agent.message_ids` is a self-documented anti-pattern in a
  shipped, funded product; our model-visible context is a pure fold bounded
  by the newest `Compacted` marker ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 8), so there is no
  second piece of state that can drift from the log or race against a
  concurrent writer.
- **Optimistic concurrency is applied by classification, not by accident.**
  Letta's OCC covers one model (`Block`) chosen, as far as the dossier
  shows, without an explainable selection principle, while the actual hot
  mutable pointer has none. Our `WRITE_PRECONDITION` table (decision 2)
  names every invariant-bearing transition explicitly and states *why* each
  one needs `At`; nothing is guarded by omission or left to be discovered
  the hard way.
- **Dedup identity never lives on domain data.** Letta's `otid` is a field
  on the message payload itself (`letta/schemas/message.py`); our
  idempotency key lives strictly on the command ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 2), never
  on the event, so a payload never has to carry its own retry-identity
  concern mixed in with its business meaning.
- **Fork-by-reference was independently validated.** Letta's
  `fork_conversation` links the *same* `Message` rows into a new
  `Conversation` via new junction rows rather than copying them
  (`letta/services/conversation_manager.py:105-174`) -- precisely the
  reference-not-copy principle [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 5 chose and defended in
  Alternatives against a physical O(history) copy. Two structurally
  unrelated designs converging on the same answer is stronger evidence for
  that choice than either alone.
- **No implicit orphan on delete.** Letta's `archives_agents` junction only
  removes the attachment row on cascade, never the `Archive` itself
  (`letta/orm/archives_agents.py:23-24`), and no code path calls the
  archive's hard delete from agent deletion -- so an archive silently
  outlives every agent that ever referenced it. Our `ArtifactErased` is
  always an explicit, separately-invoked event (`artifact_erased.proto`);
  nothing in our design can become an orphan by omission the way an
  unreferenced-but-still-live archive can in Letta.
- **An explicit, three-tier privacy contract exists at all.** Letta has no
  scheduled retention/TTL job and no redaction concept anywhere in the
  dossier -- deletion is either a real cascading hard delete
  (`ondelete="CASCADE"` on `AgentMixin.agent_id`, `letta/orm/mixins.py:40`)
  or nothing. [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 7 gives three distinct, named operations --
  `SessionHidden` (visibility only), `RedactionApplied` (read-time mask),
  `ArtifactErased` (byte destruction) -- none of which is a euphemism for the
  others, and all of which keep the log itself intact.

## Trade-offs, not gaps

- **One linear turn sequence per Session versus concurrent conversations
  under one agent.** Letta's `Conversation.id` lets multiple concurrent
  message threads share one `Agent`'s memory and identity
  (`letta/orm/conversation.py`, docstring: "Conversations that can be
  created on an agent for concurrent messaging"). A Session in our model has
  exactly one turn sequence; concurrent independent threads on the same
  underlying assistant would need either multiple Sessions or the deferred
  symmetric `Conversation` aggregate the Non-Goals name. Letta's answer
  buys concurrent-thread ergonomics at the cost of the shared-mutable-state
  problem this whole comparison is about (the context pointer); ours avoids
  that cost by not offering the feature yet.
- **Real ACID transaction for entity cascade versus a reconciler.**
  `AgentManager.delete_agent_async` deletes a manager agent and every
  sleeptime participant it owns inside one database transaction
  (`letta/services/agent_manager.py:1379-1394`), rolling back together on
  failure -- genuinely atomic, because everything lives in one Postgres
  instance. Our terminal cascade ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 6) is deliberately
  eventually consistent, because JetStream offers no atomic write across
  subjects (Alternatives Considered: "Subagent cascade via a cross-stream
  transaction or atomic multi-stream delete... rejected because it is
  unavailable"). Letta's atomicity is a property of colocated storage, not
  a design decision available to us; the trade-off we accepted (O(depth)
  reconciler round-trips) is the honest cost of a topology that does not
  offer Letta's shortcut.
- **A hard, cascading delete versus keep-forever plus masking.** Deleting a
  Letta agent really deletes its messages and conversations
  (`ondelete="CASCADE"` on `AgentMixin.agent_id`, `letta/orm/mixins.py:40`);
  there is no intermediate option between "keep everything, unmasked,
  forever" and "destroy it all." [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 7 buys a middle ground --
  masked-but-present, or byte-erased-but-provenance-kept -- at the cost of a
  more complex privacy model with three distinct operations to reason about
  instead of one.

## What not to copy

- **A mutable, unguarded pointer standing in for a derivable fold.**
  `Agent.message_ids` / `ConversationMessage.in_context` is the anti-pattern
  the whole comparison is built around; Letta's own source names it as such.
  If our model-visible-context compilation is ever "optimized" by caching
  a directly-writable current-window field, it must not be optimized this
  way -- see recommendation 2.
- **Guarding one entity with OCC while leaving the actual hot mutable state
  unguarded.** `Block.version_id_col` being the *only* OCC-checked model,
  while `Agent.message_ids` updates are last-write-wins with no check at
  all, is worse than having no OCC anywhere, because it invites the false
  belief that concurrency is handled. See recommendation 4.
- **Letting a resource become an implicit orphan through a partial
  cascade.** `archives_agents`'s cascade removes only the attachment row,
  never the archive; nothing calls `delete_archive_async` from
  `delete_agent_async`. An archive with no remaining agent reference is a
  silent, permanent leak with no error and no cleanup path found in the
  dossier. Our `ArtifactErased` must stay an explicit, separately-decided
  event, never an implicit side effect of some other entity's deletion.
- **Enabling a derived search index without a backfill contract.**
  Turbopuffer's per-archive `vector_db_provider` decision, made once and
  cached permanently, with no reindex path for data that predates it or a
  later policy change, is a designed-in blind spot. See recommendation 3.
- **Treating a git-backed side system as source of truth for data the
  primary store also holds, without naming which one wins.** Once
  git-memory is enabled, `Block.value` in Postgres is explicitly a cache
  and git is authoritative (`letta/services/block_manager_git.py:571`).
  [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 3's four-records-with-separate-authority discipline
  exists precisely to prevent this kind of ambiguity from arising in our own
  design; it should stay that way rather than being loosened for a future
  feature that wants a similar external source of truth.

## The two gaps the industry has not closed

### Subagent cascade

[ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 6 already takes a position: a child session is its own
logical stream, linked by facts recorded on each side
(`DelegationDispatched`/`ParentLinked`); terminal cascade is driven by a
reconciler reacting to Session-level terminal markers, appending a distinct
atomic `[ParentTerminated, SessionCancelled]` batch per eligible child;
rewind invalidation is a separate, distinct batch
(`[ParentHistoryInvalidated, SessionCancelled{reason =
PARENT_REWIND_CASCADE}]`) governed by `CascadePolicy`; and acyclicity holds
by construction because `DispatchDelegation` always mints a fresh
`child_session_id`. The question here is whether Letta's evidence validates,
challenges, or refines that position, not whether we still need one.

**What Letta does, and why it only partially answers the question.**
Letta's closest analogue to a "subagent" is not a dispatched, terminating
unit of work at all: it is the **sleeptime agent**, a background
memory-management agent linked to a "main" agent via a `Group` row
(`letta/orm/group.py:1-43`) with `manager_type = sleeptime` or
`voice_sleeptime`. Sleeptime agents are first-class, sibling `Agent` rows
that persist indefinitely alongside the main agent -- there is no dispatch
event, no operation ledger, and no terminal outcome for a sleeptime agent
the way there is for one of our delegated children. This is a genuine
structural mismatch: Letta has no concept of a bounded, one-shot delegated
child session at all, so its evidence cannot directly validate or challenge
decision 6's dispatch/detach saga machinery, which exists specifically for
that case.

What it *does* offer is evidence about deletion cascade, and it is
instructive. `Group.manager_agent_id` is a foreign key with
`ondelete="RESTRICT"` (`letta/orm/group.py:24`) -- the database itself
refuses to delete a manager agent while a group still references it,
forcing the cascade through application code. `AgentManager.delete_agent_async`
(`letta/services/agent_manager.py:1320-1397`) is that application-level
cascade: deleting a sleeptime agent directly deletes its `Group` and clears
the main agent's `enable_sleeptime` flag; deleting the main agent loads
every sleeptime participant into the same deletion batch and deletes the
group too, all inside one transaction that rolls back together on failure
(`agent_manager.py:1340-1341, 1365-1394`). The comment at the call site is
explicit about why: "Handle case where we're deleting a sleeptime agent (not
the main agent). In this case, we need to clean up the group and the main
agent's enable_sleeptime flag" (`agent_manager.py:1340-1341`).

**Does this validate, challenge, or refine decision 6?** It refines one
part and is silent on the rest. Letta independently arrived at the same
structural conclusion decision 6's Alternatives section reaches for a
different reason: a blind, database-native cascade (a plain `ON DELETE
CASCADE`) is not safe for a parent-child relationship with real invariants
to maintain, and application-level orchestration has to own the cleanup
instead. Letta chose `RESTRICT` plus app-level orchestration where a naive
`CASCADE` was available to it; we chose a reconciler process manager because
JetStream offers no cross-stream atomic write at all (Alternatives
Considered: "unavailable... JetStream offers no atomic write across
subjects"). The reasons differ, but both designs reject "let the storage
substrate cascade blindly" -- that convergence, from two unrelated starting
points, is a mild point in decision 6's favor. Where Letta's evidence does
not reach is depth, width, or eventual-consistency: sleeptime agents are not
nested (`Group.agent_ids` is a flat list, `letta/orm/group.py:36`, not a
recursive tree), so Letta offers no evidence at all about the fanout or
depth concerns already on record from other products in this corpus, and
because Letta's cascade is a single ACID transaction rather than a
reconciled saga, it offers no evidence about crash-mid-cascade behavior
either. Letta neither validates nor challenges decision 6's transitive,
eventually-consistent cascade design; it is simply evidence for a narrower,
adjacent claim (blind DB cascade is unsafe for this class of relationship)
that decision 6 already assumed.

### Retention on an unbounded log

[ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 7 already takes a position: keep-forever, with
`SessionHidden` as a visibility tombstone, `RedactionApplied` for read-time
masking, `ArtifactErased` for out-of-band artifact-byte destruction, and
aggregate snapshots that "bound replay, not storage." The question is
whether Letta's evidence validates, challenges, or refines that design.

**What Letta does.** No scheduled retention or TTL-cleanup job exists
(`letta/jobs/scheduler.py`, grepped for `TTL`/`expire`/`retention`/
`cleanup`, with only an unrelated shutdown log line at `scheduler.py:228`
matching) -- deletion is entirely caller-driven. Separately, and
independent of any explicit retention policy, Letta's own context-shrinking
mechanism never deletes durable history: `reset_messages_async`'s docstring
states plainly that clearing an agent's context "does not delete them from
the database" (`letta/services/agent_manager.py:1686`), and the
`Summarizer`'s eviction path (`letta/services/summarizer/summarizer.py:136-243`)
commits its output by rewriting the context pointer, never by deleting rows
from `messages`. Reads scale by cursor, not by table size:
`list_messages` paginates on `sequence_id` with `after`/`before` semantics,
not an offset (`letta/services/message_manager.py:1001-1024`), so resume and
listing cost does not degrade as a `messages` table grows without the
code-enforced cap the dossier confirms does not exist.

Where Letta's retention story stops short of ours is granularity of
deletion, not the keep-forever direction itself. The only deletion path the
dossier documents is a real, hard, cascading delete at the whole-`Agent`
level (`ondelete="CASCADE"` on `AgentMixin.agent_id`,
`letta/orm/mixins.py:40`) or a reference-counted soft-delete scoped to one
`Conversation`'s messages (`letta/services/conversation_manager.py:556-609`).
There is no analogue anywhere in the dossier to masking specific events'
content while keeping them in the log (`RedactionApplied`), and no analogue
to destroying specific artifact bytes while retaining their provenance
(`ArtifactErased`). Letta's model gives a caller exactly two choices: keep
everything, unmasked, forever, or hard-delete the whole agent and everything
under it.

**Does this validate, challenge, or refine decision 7?** It validates the
keep-forever direction and the "bound the *read* cost, not the log size"
principle -- Letta independently converged on both, as a real production
architecture rather than a proof of concept, and its cursor-paginated read
path demonstrates the same shape works at scale without a corroborating
growth-failure report the way Cline's dossier has one. It refines decision 7
by sharpening exactly what the added complexity of a three-tier privacy
contract buys over the industry's evident default: Letta, the corpus's most
mature server-side, multi-tenant store, still has nothing between "keep it
all in full" and "delete it all," which is weaker than what decision 7
specifies, not stronger. This is presented as validation that the
finer-grained contract is worth its complexity, not as a claim that Letta's
coarser model is wrong for Letta's own problem -- a single-tenant-per-agent
memory store may simply not need the granularity a multi-session platform
does.

One cost decision 7 does not explicitly bound is worth naming here rather
than assuming away, because Letta's own `messages` table shares the same
unstated bound: nothing in the ADR states who is responsible for keeping
the *tail after the newest snapshot* from growing unboundedly long if
compaction is deferred or never triggers, the same open question already on
record from this corpus's other server-side comparisons. Letta's evidence
neither confirms nor refutes that this is a real problem for us; it simply
shows that a server-side store can ship for a long time on cursor-paginated
reads alone without it becoming visibly urgent, which is not the same claim
as "it is bounded."

## Open questions for the ADR

1. Does the platform need a stable, cross-Session "agent identity" the way
   Letta's `Agent.id` spans arbitrarily many `Conversation`s and `Run`s, or
   is fork-from-the-last-Session (`SessionForked.source_session_id`,
   decision 5) intended to be the only continuity mechanism a caller ever
   needs? (Recommendation 1.)
2. Should [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 8 state explicitly that no future
   model-visible-context optimization may introduce a stored, directly-
   writable "current window" field, given that Letta's own team apparently
   introduced exactly that field with good intentions and no concurrency
   guard? (Recommendation 2.)
3. What backfill/reindex contract will a future full-text or vector-search
   projection (decision 8's "separate, independently bootstrapped
   projection") be required to satisfy before it ships, given Letta's
   confirmed permanent blind spot for data that predates Turbopuffer's
   enablement? (Recommendation 3.)
4. Should the ADR name, explicitly, that relaxing `At` to `Any` on any
   currently invariant-bearing transition for performance reasons is
   rejected on principle, using Letta's asymmetric OCC coverage (one model
   guarded, the actual hot mutable pointer unguarded) as the citation?
   (Recommendation 4.)
5. Letta's `Conversation.id` gives one persistent agent several concurrent
   message threads sharing its memory; our Non-Goals already defer a
   symmetric multi-party `Conversation` aggregate. Does that deferred
   aggregate need to support *concurrent* threads under one continuity
   identity (question 1), and if so, does the answer to question 1
   determine the shape of that future aggregate rather than the reverse?

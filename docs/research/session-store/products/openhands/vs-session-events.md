# OpenHands compared to our session event catalog

Part of Session Store Research.
Produced by running [RESEARCH_PROMPT_COMPARISON](../../RESEARCH_PROMPT_COMPARISON.md).
Stage-one dossier: [OpenHands](./index.md).
Compared against `proto/trogonai/session/sessions/v1alpha1/` and ADR#0035 on 2026-08-04.

**Store maturity: 9/12** -- evolution scars 2/3 (a real back-compat read path,
`_effective_parent_id`, for pre-tree events, and a widened `EVENT_NAME_RE`
shipped as a corruption fix; but no schema-version field anywhere in the
event/state format and the O(N²) append-cost bug is still unfixed at the
pinned commit), operational age 3/3 (three independently filed, numbered
issues with quantified failure data: #3926 corruption at 100k events, #3906 a
measured 33x slowdown at 2,000 events, #1824 a maintainer account of
30,000-event conversations degrading to unusable), exposure 2/3 (a
vendor-shipped, actively developed product with a documented multi-host
`ConversationLease` and an explicit NFS-unreliability disclaimer on its
locking primitive, but no evidence in the dossier of paid-tier scale
comparable to a hosted product with SLA-backed resume guarantees), design
independence 2/3 (the SDK and agent-server stores are original OpenHands
designs, not forked from an upstream session-store project, but the dossier
does not establish how much of the design was carried over unchanged from an
earlier in-house iteration, so full independence is inferred, not confirmed).

## The one structural difference everything else follows from

OpenHands splits session state across two records with **separate,
non-overlapping authority**: `events/` (append-only, one file per event, the
sole record of history) and `base_state.json` (a single mutable document,
rewritten whole on every save, holding the agent snapshot, workspace binding,
`leaf_event_id` HEAD pointer, running stats, secret registry, and free-form
tags). Neither file is derivable from the other. The dossier is explicit that
`base_state.json` "cannot be rebuilt from `events/` alone," because agent
config, secrets, and tags "have no event representation" at all, and that
`events/` cannot be reconstructed from `base_state.json`, because it holds
only the current HEAD, not history.

ADR#0035 rejects this shape twice over: decision 1 makes the event log the
only place a session's state is committed, and decision 8 states "no read
model is authoritative" -- every projection, snapshot, or checkpoint is
disposable and rebuildable by replay. OpenHands' `base_state.json` is not a
disposable projection; it is a second permanent source of truth that a full
replay of `events/` cannot recover.

Walking through what actually lives in that second authority, category by
category, is more informative than treating it as one gap:

- **Agent/runner configuration.** We already fold this into the log:
  `SessionStarted.execution_plan` (a `StoredSessionExecutionPlan`, see
  `proto/trogonai/session/sessions/v1alpha1/execution_plan.proto`) is an
  event, fully replayable. No gap.
- **Secrets.** OpenHands stores a secret registry inside `base_state.json`
  itself, a second durable location holding data the event log deliberately
  never receives. We do not have a parallel secret store because we take a
  structurally different approach at facet 7: an ingress rule keeps secrets
  out of the durable log altogether (confirmed by grep -- the only two
  `secret` hits in the entire 57-file catalog are the comments on
  `ExternalArtifact.source_url` in
  `proto/trogonai/session/sessions/v1alpha1/artifact.proto:79-80`,
  documenting that source URLs must be credential-free before they are
  recorded). This is not a smaller version of OpenHands' problem; it removes
  the reason a second authoritative store would be needed for secrets in the
  first place.
- **Tags.** Grepping the catalog (`grep -rniE "\btag"
  proto/trogonai/session/sessions/v1alpha1/`) returns no field or event
  anywhere. The catalog has `SessionRenamed`, `SessionArchived`, and
  `SessionUnarchived` for organization state, but nothing for free-form
  categorization. This is a genuine, narrow gap -- see recommendation 2.
- **The `leaf_event_id` HEAD pointer.** This is the one category where
  OpenHands' split buys something real: a movable pointer that lets a caller
  jump to any prior event and grow a new sibling branch from it
  (`navigate_to()`), all within one conversation id. We do not have an
  equivalent, and do not think we need one -- see "Trade-offs, not gaps"
  below -- because our fork (`SessionForked`, decision 5) mints a new
  aggregate by reference rather than moving a pointer within one aggregate,
  and our rewind (`SessionRewound.keep_through`, decisions 2 and 6) is a
  forward-appended marker interpreted at replay, not an in-place
  re-rooting operation.

So the honest read is not "OpenHands has a second authoritative store and we
don't" as a blanket win. It is: two of the four categories that force
OpenHands' split (config, secrets) are already solved without one in our
design, one (HEAD) is solved by not needing the operation it enables, and one
(tags) is a real, small, additive gap we should close.

## Mapping

| OpenHands construct | Our equivalent | Verdict |
| --- | --- | --- |
| `events/` -- append-only dir, `event-{idx:05d}-{id}.json` | Logical stream per session on the shared `SESSION_EVENTS` JetStream stream (ADR facet 1) | Ours -- broker-native, no filename-encoded index |
| `base_state.json` (agent snapshot, workspace, HEAD, stats, secrets, tags) | No single equivalent -- see structural-difference section above | Split verdict: ours ahead on config/secrets, gap on tags, HEAD not needed |
| `Event.id` + in-memory `_id_to_idx`, linear id-seen check before append | `Event.id` deterministically derived from `(stream subject, command type, idempotency key, batch index)` (ADR facet 2) | Ours -- principled derivation, not a dedup scan |
| Filename-encoded index = order (`event-{idx:05d}`) | `SessionOrdinal`, fold-derived, not physically assigned (`proto/trogonai/session/sessions/v1alpha1/session_ordinal.proto`) | Ours -- survives restore/backfill/cold-tier relocation; theirs is coupled to the filename |
| `parent_id` event tree + `navigate_to()` / `fork()` | `SessionRewound.keep_through` (forward marker) + `SessionForked` (new aggregate) | Trade-off -- see below |
| `ConversationLease` (`owner_lease.json`: TTL, generation, host/PID) | JetStream's native `Nats-Expected-Last-Subject-Sequence` compare-and-swap for `At`-guarded commands (ADR facet 2) | Ours -- no filesystem lease needed |
| `Condensation` event (`forgotten_event_ids`, `summary`, `summary_offset`, `llm_response_id`) | `Compacted` (`covers_from`, `covers_through`, `trigger`, `guidance`, `tokens_before`, `tokens_after`, `usage`; `proto/trogonai/session/sessions/v1alpha1/compacted.proto`) | Ours records token/usage provenance theirs does not; theirs permits a non-contiguous `forgotten_event_ids` set where ours requires a contiguous range -- see Open questions |
| `CondensationSummaryEvent`, regenerated in-memory from `Condensation.summary` on every view rebuild | `Compacted.summary_content`, stored inline once | Trade-off -- see below |
| SDK `TaskManager` Task tool: in-process `task_id` counter, `TaskObservation` summary event | No ephemeral/lightweight delegation primitive -- every delegation is a first-class `Session` via `DelegationDispatched` / `ParentLinked` | Deliberate difference -- see "the two gaps the industry has not closed" |
| Agent-server `parent_conversation_id`: client-supplied, orphan-not-cascade on delete | `DelegationDispatched` / `ParentLinked` / `CascadePolicy` (`proto/trogonai/session/sessions/v1alpha1/delegation_dispatched.proto`, `parent_linked.proto`, `cascade_policy.proto`) | Ours, decisively -- typed, reconciler-driven cascade or independence, recorded as a fact at dispatch time |
| `sub_conversation_ids` -- derived via full linear scan of the catalog on every call | Parent-to-children lineage projection folded from `DelegationDispatched` (ADR facet 6, facet 8) | Same principle (derived, not stored); ours is incrementally checkpointed, theirs re-scans every call |
| No detach/undelegate concept; only orphan-on-delete | `DelegationDetached` / `ParentDetached`, a two-fact saga joined by `detach_operation_id` (`delegation_detached.proto`, `parent_detached.proto`) | Ours, decisively -- a gap in theirs |
| `usage_to_metrics[f"task:{task.id}"]` -- in-memory-only cost rollup | No rollup field; delegation cost is recoverable by folding the child's own stream | Ours, deliberately -- see "What not to copy" |
| No `schema_version` on `Event` or conversation state; `PersistedSettings.schema_version` exists for a sibling subsystem | No per-event version field either; additive-only evolution (ADR decision 3); read-model versioned at the package level, `projections/v1` (ADR facet 8) | Validated, not gapped -- see "What our design already does better" |
| `LocalFileStore`: no atomic temp-file-rename, `FileLock`-based locking, documented as unreliable on NFS | JetStream durable log; broker-native atomicity, no filesystem lock | Ours |
| Fork = deep-copy (JSON round-trip) of the full or branch-sliced event set into a new conversation directory | `SessionForked` = O(child events) only, inherits by reference via context projection (ADR decision 5) | Ours, decisively |
| No turn/round concept; only per-event `parent_id` tree position | `turn_id` stamped on `UserMessageRecorded`, all `AssistantMessage*` events, and `ToolCallRequested` / `Started` / `Completed` / `Failed` (ADR decision 3) | Ours, decisively |
| No workspace relocation/rebind reconciliation found anywhere in the reviewed source | `WorkspaceRef` immutable per session; rebind requires a new session or fork (`proto/trogonai/session/sessions/v1alpha1/workspace.proto`) | Ours -- same answer as most of the corpus, more explicit |
| `EventLog` itself has no clear/delete/prune method; `delete_conversation` at the service layer *does* remove the conversation's directory outright | `SessionHidden` / `RedactionApplied` / `ArtifactErased` -- three graduated, distinct operations (ADR decision 7) | Mixed -- see "What not to copy" |
| `_count_events_on_disk()` -- full directory listing on every append, O(N²) across a session's life (#3906) | Server-side `Nats-Expected-Last-Subject-Sequence` compare-and-swap, O(1) at the broker, for `At`-guarded commands; no guard at all for `Any`-guarded commands | Ours, decisively -- see "the two gaps the industry has not closed" |
| `search_conversations` / `count_conversations`: full linear scan over all conversations per call; `sub_conversation_ids`: full linear scan per call | `SessionProjection`, a KV-backed read model incrementally checkpointed after each event via `Projector::catch_up` (ADR decision 8) | Ours -- incremental vs. full-scan-per-request |
| Free-form `tags` in `base_state.json` | No equivalent anywhere in the catalog (grep-confirmed) | Gap -- see recommendation 2 |

## What we should consider changing

### 1. Do not adopt a second, non-replayable authoritative store

**The change** this would be: adding a `base_state.json`-style document
outside the event log to hold per-session config, secrets, or metadata that
the log itself does not carry -- i.e., contradicting ADR decision 1
("every retroactive operation is a new appended event") and decision 8
("no read model is authoritative").

**Evidence anchor**: OpenHands dossier (store maturity 9/12), the
`base_state.json` / `events/` split described in the structural-difference
section above, specifically that agent config, secrets, and tags "have no
event representation" and cannot be rebuilt from `events/` alone.

**Blast radius**: Breaking the decision (ADR#0035 decisions 1 and 8).

**Why not**: walking through OpenHands' own four categories shows the split
buys them very little that a well-designed event log can't already do more
cheaply. Config already folds from `SessionStarted.execution_plan`. Secrets
are better solved by never admitting them to the log (our ingress rule) than
by admitting them to a *different* durable document that still has to be
protected, backed up, and kept in sync with restore/migration tooling. The
only thing their split earns that ours structurally can't is movable HEAD
navigation, which is a different trade-off (see below), not a reason to
duplicate authority. A second authoritative store also reintroduces exactly
the "which one wins" ambiguity ADR#0035 was written to remove -- the dossier's
own account of `_resolve_active_leaf()` falling back to a best-effort scan
when the HEAD pointer is unset or stale is a symptom of that ambiguity, not a
feature of it.

**Cost beyond migration**: a second store means a second backup path, a
second consistency check on resume, and a second thing that can drift from
the log during a partial write -- the exact failure class decision 1 exists
to close off.

### 2. Add a generic session-tags concept

**The change**: a new event, e.g. `SessionTagged` (or a `repeated string
tags` field folded the same way `SessionRenamed`/`SessionArchived` already
are), mirroring the existing "reversible listing-state" pattern the catalog
uses for session organization.

**Evidence anchor**: OpenHands' `base_state.json.tags` field (per the
dossier's account of `state.py`), a real, shipped, user-facing categorization
mechanism with no event representation on their side either; and our own
grep of `proto/trogonai/session/sessions/v1alpha1/` returning zero hits for
any tag-like field.

**Blast radius**: Additive.

**Why**: this is cheap and low-risk precisely because the pattern already
exists in the catalog (`SessionRenamed`, `SessionArchived`,
`SessionUnarchived`) -- tags are one more piece of user-facing listing
metadata, not a new kind of state. It closes the one category from the
structural-difference analysis that is a genuine gap rather than a
solved-differently problem.

**Cost beyond migration**: one more event type to fold into
`SessionProjection`, and a decision (left open below) on whether tag mutation
should be its own event or ride along on an existing organization event.

### 3. Do not add a delegation-cost rollup field to `OperationOutcomeRecorded`

**The change** this would be: adding an aggregated `TokenUsage`/cost field to
`OperationSucceeded` (`proto/trogonai/session/sessions/v1alpha1/operation_outcome_recorded.proto`)
so a parent gets a rolled-up cost for a completed delegation without folding
the child's stream.

**Evidence anchor**: OpenHands' `usage_to_metrics[f"task:{task.id}"]`
rollup (per the dossier), which aggregates a sub-agent's LLM usage into the
parent's stats under a task-id key.

**Blast radius**: Additive (if done) / no-op (rejected).

**Why not**: OpenHands' rollup is in-memory only -- not durable, not part of
`events/`, and lost on restart unless recomputed. It is weaker evidence for
the field than it looks. More importantly, adding a denormalized rollup
contradicts the discipline the rest of our own catalog already follows: cost
and usage are recorded once, per message, on the stream that produced them
(`TokenUsage`, decision 3), and anything that needs an aggregate is expected
to fold the relevant stream rather than carry a second, potentially-stale
counter. `OperationSucceeded` already carries `response_digest` and an
optional `response_ref`; it should stay a receipt, not a ledger.

**Cost beyond migration**: none avoided -- this is a "do not do this" entry
specifically so the idea isn't re-proposed the next time someone reads
OpenHands' `usage_to_metrics` and wants to imitate it.

## What our design already does better

- **Turn identity is a stamped fact, not an inferred one.** OpenHands has no
  turn/round concept at all -- only per-event `parent_id` tree position, which
  encodes branch topology, not conversational turn boundaries. Decision 3's
  `turn_id` on `UserMessageRecorded`, `AssistantMessage*`, and the
  `ToolCallRequested`/`Started`/`Completed`/`Failed` family exists precisely
  because concurrent `Any`-precondition appends give no reliable "next event
  after" relation to infer membership from -- a problem OpenHands' tree
  doesn't need to solve because it has no turn concept to begin with, not
  because it solved it more cheaply.
- **Fork is O(child events), not a deep copy.** OpenHands' fork performs a
  JSON round-trip of the full (or branch-sliced) event set into a new
  conversation directory -- an actual physical copy. `SessionForked` (decision
  5) inherits by reference via a context projection; replay cost is bounded
  by the child's own events regardless of fork depth or how large the parent
  became before the fork.
- **Cascade and detach are typed, reconciler-driven, and recorded as facts --
  not left as a side effect of deletion.** Both of OpenHands' delegation
  mechanisms orphan children rather than cascade: the agent-server's
  `parent_conversation_id` is explicitly left dangling on parent delete (the
  dossier quotes the code comment: children are "orphaned, not cascaded"),
  and the SDK's Task-tool subagent directories are documented as never
  garbage-collected when the parent conversation persists. Decision 6's
  `CascadePolicy` (`CASCADE_ON_PARENT_TERMINAL` / `INDEPENDENT`) makes this an
  explicit, typed, per-delegation fact recorded at dispatch time, with a
  reconciler that repairs the link after a crash -- not an implicit
  consequence of whichever code path happens to run at delete time.
- **Concurrency control is broker-native, not a filesystem scan.** OpenHands'
  `_count_events_on_disk()` performs a full directory listing on every
  append to detect a stale index -- an O(N²) cost across a session's life,
  still unfixed at the pinned commit (#3906, a measured 33x slowdown at
  2,000 events). Our `At`-guarded commands use JetStream's native
  `Nats-Expected-Last-Subject-Sequence` header, an O(1) broker-side
  compare-and-swap (decision 2); `Any`-guarded commands carry no server-side
  position check at all. Neither path has an analogue to a client-side
  listdir.
- **Privacy is graduated and explicit, not a single destructive delete.**
  OpenHands' `delete_conversation` removes the conversation's directory
  outright -- an irreversible deletion with no distinction between "hide from
  listings" and "erase the bytes." Decision 7 replaces the single
  `SessionDeleted` concept with three distinct, ordered operations --
  `SessionHidden` (visibility tombstone), `RedactionApplied` (read-time
  masking, bytes remain), `ArtifactErased` (out-of-band byte destruction,
  digest and metadata remain as provenance) -- specifically because a single
  "delete" conflates operations with very different guarantees.
- **The read model is versioned where OpenHands independently agrees it
  should be, and unversioned where OpenHands independently agrees it should
  be.** OpenHands has no `schema_version` on `Event` or conversation state,
  but does have one on the sibling `PersistedSettings` document -- a
  mutable, whole-document read-modify-write structure, unlike the
  append-only event log. That is exactly the split ADR decision 3 (additive,
  unversioned event evolution) and decision 8 (`SessionProjection` versioned
  at the package level, `projections/v1`) already encode. OpenHands arriving
  independently at the same split is evidence *for* decision 3, not a gap in
  it -- see "Open questions" for the one nuance worth flagging.

## Trade-offs, not gaps

- **Movable HEAD / in-place branch navigation vs. no in-aggregate branch
  switching.** OpenHands' `leaf_event_id` in `base_state.json` lets a caller
  `navigate_to()` any prior event and grow a new sibling branch from it, all
  within one conversation id, with earlier branches still on disk and
  re-selectable later. Our model has no equivalent: revisiting an earlier
  point in a session's history means either a forward rewind marker
  (`SessionRewound.keep_through`, not reversible except by another rewind)
  or a fork into a brand-new session id (decision 5). OpenHands buys
  cheap, repeated, non-destructive exploration of a single identity's history
  at the cost of a movable, out-of-band pointer that must be persisted,
  recovered on crash (the dossier's `_resolve_active_leaf()` fallback,
  referencing bug #4057, for when that pointer is unset or stale), and kept
  distinct from the append-only log it points into. We buy a purely
  fold-derived position (`SessionOrdinal`, decision 2) with nothing extra to
  persist or recover, at the cost of every branch exploration outside a
  single rewind becoming a new session identity rather than a revisitable
  node in one tree. Neither is strictly better; they answer different
  questions about what "one session" is allowed to mean.
- **Contiguous compaction range vs. arbitrary forgotten-event set.**
  `Compacted.covers_from`/`covers_through` (decision 4) is a contiguous
  inclusive range, validated as such (`covers_from <= covers_through`).
  OpenHands' `Condensation.forgotten_event_ids` is a set, which is
  structurally capable of expressing a non-contiguous forgetting policy
  (keep some early events, drop a non-contiguous middle segment, keep
  recent ones). The range form is simpler to validate and query; the set
  form is more expressive. Whether that extra expressiveness is ever
  exercised in practice is unconfirmed -- see Open questions.

## What not to copy

- **A second permanently authoritative document alongside the log**
  (`base_state.json`). Even setting aside decisions 1 and 8, it is a second
  thing that must be backed up, migrated, and kept consistent with the log
  it cannot be derived from or rebuild.
- **Client-side, unindexed directory listing as a concurrency check**
  (`_count_events_on_disk()`). This is the clearest anti-pattern in the
  dossier: an O(N²) cost across a session's life, filed as #3906, still
  unfixed at the pinned commit.
- **In-memory-only cost rollups** (`usage_to_metrics[f"task:{task.id}"]`) as
  a substitute for folding the source of truth. Not durable, and duplicates
  what a projection can already compute by folding the child's own stream.
- **Physically deleting the event directory on `delete_conversation`** with
  no masking or erasure distinction. This forecloses audit and rewind
  entirely and collapses exactly the "hide vs. redact vs. erase" distinction
  decision 7 was written to preserve.
- **A cleanup flag that is silently inert.** The dossier notes
  `delete_on_close=True` is accepted as a parameter but never checked by the
  code path that would need to act on it (`LocalConversation.close()`). A
  control surface that looks like it does something and does nothing is
  worse than no control surface at all.
- **Two structurally different, non-overlapping delegation mechanisms**
  (the SDK's ephemeral Task tool and the agent-server's
  `parent_conversation_id`) that both still leave children orphaned rather
  than cascaded, and whose subagent directories are never garbage-collected
  when the parent persists. Pick one coherent model; decision 6 already did.
- **Filesystem locking with a documented network-filesystem disclaimer**
  used for the primary event log. The `ConversationLease` design (TTL,
  monotonic generation, PID liveness) is sound engineering, but it sits on
  top of `LocalFileStore`'s `FileLock`, which the dossier notes is
  explicitly documented as unreliable on NFS. A broker-native log removes
  the need for a filesystem lease altogether.

## The two gaps the industry has not closed

### Subagent cascade

OpenHands' evidence tests decision 6 twice, on two independent mechanisms,
and both times lands on the same answer: orphan, don't cascade. On the
agent-server side, the dossier quotes the code path directly -- deleting a
parent leaves `parent_conversation_id` dangling on any children, described
in-code as "orphaned, not cascaded," the same treatment given to
`forked_from_conversation_id` on source deletion. On the SDK side, a
sub-agent's own crash inside `_run_task()` is caught, recorded as
`task.error`, and reported to the parent via `TaskObservation(is_error=True)`
-- but whatever the sub-agent had already durably appended to its own
`events/` remains on disk, untouched, with no rollback and no cascade signal
beyond that one observation. Neither mechanism has any documented behavior
for a *parent* rewind cascading to a live child, and neither has a
crash-of-the-parent-host cascade path distinct from the parent's own
in-process exception handling -- the dossier's crash-recovery machinery
(`ConversationLease`) governs which process owns a conversation, not what
happens to that conversation's children when it is lost.

This validates, rather than challenges, decision 6. The ADR's own comment on
`CascadePolicy` in
`proto/trogonai/session/sessions/v1alpha1/cascade_policy.proto` calls
`CASCADE_ON_PARENT_TERMINAL` the "safe default" precisely because
orphan-by-default -- the choice both of OpenHands' independent delegation
mechanisms make -- is the failure mode it exists to prevent. The dossier's
observation that Task-tool subagent directories are never garbage-collected
when the parent persists is a concrete instance of exactly the drift decision
6's typed, reconciler-repaired cascade (`ParentTerminated`/
`ParentHistoryInvalidated`/`SessionCancelled`) and decision 7's
`SessionHidden`-based visibility model were designed to replace with an
explicit, recorded fact instead of a silent, permanent directory leak. Where
OpenHands refines the picture is in showing that the ADR's own
"parent-first dispatch, then link" ordering (`DelegationDispatched` →
`ParentLinked`) is not something every product bothers to get right even
once, let alone twice: OpenHands ships two disjoint answers to the same
question and neither one closes the gap decision 6 closes with one.

### Retention on an unbounded log

OpenHands confirms, with numbers, that "keep forever" without a bounding
mechanism is a shipped, user-visible failure, not a theoretical one: #3926
(event-log corruption reported at roughly 100,000 events), #3906 (a measured
33x append-time slowdown at 2,000 events, from the O(N²)
`_count_events_on_disk()` directory listing, still present at the pinned
commit), and #1824 (a maintainer's own account of conversations reaching
roughly 30,000 events and becoming unusably slow or crash-prone). Its
`Condensation` event is instructive here too: condensation is an ordinary,
durably persisted event, exactly like our `Compacted` (decision 4) -- only
the in-memory `View` shrinks; `events/` itself never does. That part of the
picture matches our own design already (decision 7 explicitly accepts that
the log grows forever and treats storage as a capacity-planning concern, not
something compaction reduces).

Testing OpenHands' three specific cost categories against decision 7 (plus
decisions 2 and 8, which is where the actual bounding mechanisms live):

- **Per-append cost.** OpenHands' failure is a client-side, unindexed,
  full-directory listing on every append. Our `At`-guarded commands use
  JetStream's native `Nats-Expected-Last-Subject-Sequence` header, an O(1)
  broker-side compare-and-swap; `Any`-guarded commands carry no server-side
  position check at all. Neither path has an analogue to a listdir. No gap.
- **Listing/projection-catch-up cost.** OpenHands' `search_conversations`,
  `count_conversations`, and `sub_conversation_ids` are each a full linear
  scan at request time, with cost proportional to total conversation count.
  Decision 8's `SessionProjection` is checkpointed after each event via
  `Projector::catch_up`, so a query's cost tracks events since the last
  checkpoint, not the size of the whole catalog. No gap, provided the
  projector is kept caught up.
- **Resume/replay cost.** OpenHands' cold resume rebuilds the active branch
  with, per the dossier, no page or cursor bound. Decision 8 states resume
  cost "tracks snapshot cadence, not transcript length." This is a real
  structural answer, but it is conditional on snapshots actually being taken
  regularly -- the ADR does not pin a maximum snapshot staleness anywhere in
  the text reviewed, and exact snapshot cadence is explicitly left to
  implementation-level follow-up in the Non-Goals. If that cadence is
  allowed to lag, replay cost degrades toward the same profile as
  OpenHands' unbounded active-branch replay. This is the one place OpenHands'
  evidence refines decision 7/8 rather than simply validating it -- see Open
  questions.

## Open questions for the ADR

- Decision 8 states that resume cost "tracks snapshot cadence, not
  transcript length," but neither the Decision nor the Non-Goals sections
  pin a maximum snapshot staleness. Given OpenHands' quantified failure
  thresholds (corruption near 100,000 events, a 33x slowdown at 2,000, and a
  maintainer-reported 30,000-event conversation becoming unusable), is a
  numeric snapshot-cadence bound worth recording now, even as a target for
  the implementation-level follow-up the ADR already defers this to, rather
  than leaving it fully open?
- Should a generic session `tags` concept (recommendation 2) be its own
  event (`SessionTagged`), or does free-form categorization belong on a
  metadata surface outside the append-only contract entirely? The catalog
  currently has no organization-state precedent for arbitrary, user-defined
  values (as opposed to the fixed `SessionRenamed`/`SessionArchived`/
  `SessionUnarchived` set).
- Is `Compacted`'s contiguous `covers_from`/`covers_through` range shape ever
  going to be insufficient for a condenser policy that wants to keep some
  early context and drop a non-contiguous middle segment in a single marker?
  OpenHands' `forgotten_event_ids: set[EventID]` permits this structurally,
  though the dossier does not confirm any condenser policy actually exercises
  non-contiguous forgetting in practice. [inference]
- Do we want any form of non-destructive, within-identity branch
  re-exploration (OpenHands' `navigate_to()`), or is minting a new session
  via fork the accepted, permanent answer for every case where a caller
  wants to revisit an earlier point and try something different? This is a
  product-scope question, not a storage-design gap -- the trade-off section
  above states what each side buys.

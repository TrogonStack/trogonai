# Google ADK compared to our session event catalog

Part of Session Store Research.
Produced by running [RESEARCH_PROMPT_COMPARISON](../../RESEARCH_PROMPT_COMPARISON.md).
Stage-one dossier: [Google ADK](./index.md).
Compared against `proto/trogonai/session/sessions/v1alpha1/` and [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) on 2026-08-04.

All ADK `path:line` citations below are repo-root-relative to the pinned clone
the dossier used (commit `cbedafd9e4c18d462dc571e1bb079177a496ef51`), exactly
as the dossier itself states, never to this docs repo. All of our own
citations are repo-root-relative to this repository.

**Store maturity: 10/12** -- evolution scars 3/3 (a real generational schema
cut-over is shipped and still supported: `adk_internal_metadata` tracks a
schema-version row, `_schema_check_utils.get_db_schema_version_from_connection`
detect-then-branches to a legacy pickle format when that table is absent
(`src/google/adk/sessions/migration/_schema_check_utils.py:70-89`),
`DatabaseSessionService` keeps parallel v0/v1 SQLAlchemy model classes live so
old databases keep working without migration
(`src/google/adk/sessions/database_session_service.py:259-276`), and a working
migration tool with a restricted, allowlisted unpickler
(`src/google/adk/sessions/migration/migrate_from_sqlalchemy_pickle.py:41-126`)
plus a written policy requiring at least two releases of back-compat before an
old branch is removed (`src/google/adk/sessions/migration/README.md:106-129`)),
operational age 1/3 (the dossier cites no issue tracker report of corruption,
growth, or lock contention anywhere in this package, and its own Open
Questions section says outright that whether `SqliteSessionService`'s cruder
staleness check is "an intentional simplification or an oversight is not
stated anywhere in the source" -- the only evidence of real field age is the
existence of live v0 databases needing migration, not a corroborated failure
report; this axis is comparatively thin evidence and should be weighted
accordingly), exposure 3/3 (four shipped backends including
`VertexAiSessionService`, a paid Google Cloud Agent Engine API, plus
explicit multi-host handling for the database backend via per-dialect
row-level locking and a connection-pooled `AsyncEngine`
(`src/google/adk/sessions/database_session_service.py:431-436`)), design
independence 3/3 (no evidence the store was forked from another product; the
dossier's own scope note says only `BaseSessionService` and its four backends
were read, and the `migration/` directory migrates ADK's own prior schema
generations, not a foreign format).

## The one structural difference everything else follows from

ADK's durable session is **session-as-document plus session-as-log under one
id, where only the log half is genuinely append-only.** The `Session` pydantic
model carries `state: dict[str, Any]` and `events: list[Event]` as siblings
(`src/google/adk/sessions/session.py:28-73`). `events` rows are only ever
inserted across all four backends. `state`, by contrast, is written *directly*
on every `state_delta`: a full-value overwrite in `DatabaseSessionService`
(`storage_session.state.update(...)`,
`src/google/adk/sessions/database_session_service.py:931-932`) or an atomic
`json_patch()` merge in `SqliteSessionService`
(`src/google/adk/sessions/sqlite_session_service.py:552-596`). No backend ever
folds `events` to reconstruct `state`; every read returns the live
column/attribute as-is. The dossier's own conclusion: `state` is
"rebuildable-in-principle but not-actually-rebuilt" (dossier, "The storage
model" section), so nothing stops it from silently drifting from what
replaying `events` would produce.

This is the exact question [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 8 answers the other way: "No
read model is authoritative. A `Projector::catch_up` folds the stream into a
`SessionProjection`..." and "the aggregate snapshot is an advisory cached fold
of that log. Corruption or incompatibility falls back to earlier replay"
([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md), facets
3 and 8). ADK is a concrete, shipped counter-example of what happens when a
"projection" is allowed a direct write path of its own instead of being purely
fold-derived: it is not a hypothetical risk decision 8 is guarding against in
the abstract, it is the load-bearing state store of a widely distributed
framework, doing exactly the thing decision 8 forecloses. Everything else in
this comparison -- the rewind marker needing a computed reversal payload
(below), the concurrency cliff varying by backend, the three subagent
storage shapes -- traces back to ADK treating `state` as a second, independently
mutable source of truth rather than a value that is always re-derivable from
`events` alone.

## Mapping

| ADK | Ours | Verdict |
| --- | --- | --- |
| `Session{id, app_name, user_id}` compound primary key (`src/google/adk/sessions/session.py:39-49`, `schemas/v1.py:75-85`) | Opaque `SessionId` addressing one subject `session.sessions.events.<session_id>` ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 1) | Semantic mismatch: ADK bakes tenant/app/user scoping directly into the primary key; we keep identity opaque and push multi-tenant scoping onto the subject prefix a resolver declares ([ADR#0027](../../../../adr/0027-decider-multi-tenancy-primitive.md)) |
| `Session.state` (mutable folded document, directly overwritten or `json_patch`'d) | No equivalent as an authoritative record; the closest concept is the aggregate snapshot, always an "advisory cached fold," never independently written ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 8) | Ours, decisively -- see structural difference above |
| `Session.events: list[Event]` | Session event stream on `session.sessions.events.<session_id>` | Equivalent shape, different typing discipline: `Event` has `extra='ignore'` and no schema validation at the storage boundary; every one of our events is schema-validated protobuf at append ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 3) |
| `EventActions.state_delta` (patched into `state` at every append) | No equivalent field; a change to session-scoped data is the fact itself (`TodoUpdated`, `FileChanged`, etc.), never a patch applied to a folded document | Ours, deliberately -- there is no document to patch, so there is nothing for a delta field to reconcile against |
| `EventActions.rewind_before_invocation_id` plus a computed reversing `state_delta` (`_compute_state_delta_for_rewind`, `src/google/adk/runners.py:1380-1405`, walks forward and diffs against current state to compute what to null out) | `SessionRewound{session_id, keep_through: SessionOrdinal, reason: RewindReason}` (`proto/trogonai/session/sessions/v1alpha1/session_rewound.proto`) | Ours, decisively -- no reversal payload to compute or persist; `keep_through` alone is sufficient because state is never a folded document that needs "reversing" in the first place |
| `_apply_rewinds` (`src/google/adk/events/_rewind_events.py:22-55`), a shared pure function every reader must remember to call | Model-visible context "compiled deterministically from the event log bounded by the latest `Compacted` marker" ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 8) | Ours, structurally, but not yet stated as the *sole* mandatory entry point for a rewind-aware read -- see recommendation 1 |
| `Event.branch` / `_BranchPath` (in-session subagent scoping by a dot-separated path string, `src/google/adk/events/_branch_path.py:20-151`) | No equivalent; every delegation gets its own logical stream (`DelegationDispatched`, `ParentLinked`, [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 6) | Trade-off, not a gap -- see below |
| `EventActions.agent_state` ("checkpoint and resume... should only be set by ADK workflow," in-band on an ordinary event) | `Checkpoint` embedded in `CheckpointProduced` / `ExecutionAttemptStarted.restored_checkpoint`, a distinct, digest-verified, out-of-line artifact with its own admission contract (`proto/trogonai/session/sessions/v1alpha1/checkpoint.proto`, `checkpoint_produced.proto`; [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 3) | Semantic mismatch: ADK's "checkpoint" is workflow state riding on the ordinary transcript; ours is a separately-authorized record with its own `covers_through`, `checkpoint_id`, and plan-digest equality checks |
| `GetSessionConfig.num_recent_events` / `after_timestamp`, a query-time `LIMIT`/timestamp filter with no cursor pagination (`src/google/adk/sessions/base_session_service.py:29-43`) | Resume from the newest aggregate snapshot, replay only the tail after it ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 8) | Ours, decisively -- bounded by snapshot cadence, not an ad hoc query-time limit the caller must remember to pass |
| `_storage_update_marker` + SQLAlchemy staleness check, present only in `DatabaseSessionService`, cruder in `SqliteSessionService`, absent in `InMemorySessionService`/`VertexAiSessionService` | `WRITE_PRECONDITION = At(current_position)` enforced server-side by JetStream for every invariant-bearing command by default ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 2) | Ours, decisively -- see recommendation 5 |
| `adk_internal_metadata` schema-version row + `_schema_check_utils` detect-then-branch, two live parallel model-class generations (pickle v0, JSON v1) | No per-event schema-version field; evolution is additive only, "never a per-event version branch" ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 3) | Trade-off -- see below |
| `migration_runner.upgrade`, one-way dump-and-reload with a restricted unpickler and a written 2+-release back-compat policy | No migration event or tooling in the catalog today | Gap, already tracked in the [fx comparison](../fx/vs-session-events.md#9-migrations-are-not-journaled), item 9; ADK is corroborating evidence -- see recommendation 4 |
| `app_states` / `user_states` tables, `app:`/`user:`-prefixed keys, never touched by `delete_session` | No equivalent scope; all state is fold-derived per-session, nothing shared cross-session in `v1alpha1` | Ours, by absence of the feature -- but see the retention section and open question 6 |
| `DatabaseSessionService.delete_session` (real SQL `DELETE`, `ON DELETE CASCADE` to `events`, `schemas/v1.py:208-213`) | `SessionHidden`, a visibility tombstone; no bytes are ever deleted ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 7) | Semantic mismatch: ADK's "delete" removes bytes in the DB backends; ours never does -- the closest ADK concept to our keep-forever contract is that `app_states`/`user_states` also survive `delete_session`, but as an unintentional orphan, not a deliberate design |
| `VertexAiSessionService` `ttl` / `expire_time` passthrough to the remote API | No TTL concept; retention is keep-forever plus explicit `SessionHidden`/`RedactionApplied`/`ArtifactErased` facts ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 7) | Trade-off -- ADK delegates retention entirely to one vendor-hosted backend rather than deciding it at the store layer |
| Three subagent storage shapes: in-session branch scoping, in-session `isolation_scope`-filtered Task-API delegates, and a fully separate throwaway `InMemorySessionService` per `AgentTool` call | One model: every delegation is `DelegationDispatched` + `ParentLinked` into a genuinely separate, durable child stream ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 6) | Ours, decisively for the throwaway case; trade-off for the two in-session cases -- see the subagent cascade section |
| `Event.timestamp` (client-generated `float`) with `id` (UUID4) as an arbitrary-but-stable tiebreak for read ordering (`database_session_service.py:697-699`) | `SessionOrdinal`, fold-derived, never a physical sequence or client clock ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 2) | Ours, decisively -- no clock-skew reordering risk for any durable cross-reference |
| `Event.id`, self-assigned client-side UUID4, deduplicating only by accidental primary-key collision on retry | `Event.id`, deterministically derived UUIDv5 over `(subject, command type, idempotency key, batch index)` ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 2) | Ours, decisively -- a designed idempotency contract, not an accidental one |
| `ListSessionsResponse` docstring claims "states not set," contradicted by three of four backends actually setting `state` (`in_memory_session_service.py:274-283`, `database_session_service.py:784-794`, `sqlite_session_service.py:357-371`) | `SessionProjection`, one documented read-model contract per aggregate -- the projection value type *is* the read contract ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 8) | Ours, decisively -- no doc/implementation drift is possible when there is exactly one authoritative contract instead of a docstring three of four backends silently violate |

## What we should consider changing

Ordered by how much is at stake, not by implementation cost.

### 1. Name the model-visible-context compiler as the sole mandatory fold point for every rewind- and compaction-aware read

**The change.** [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 8 says the model-visible context is
"compiled deterministically from the event log bounded by the latest
`Compacted` marker," which already centralizes compaction-aware folding in
one place. The decision text does not equally name `SessionRewound.keep_through`
as bounded by that same single compiler, nor does it forbid a future reader
(a search projection, a cost rollup, a UI preview) from writing its own ad hoc
check for "is this event still live after the latest rewind."

**Evidence anchor.** Google ADK, store maturity 10/12: `_apply_rewinds`
(`src/google/adk/events/_rewind_events.py:22-55`) is a shared pure fold
function that the prompt-content builder and *both* compaction policies must
independently call before doing anything else -- and ADK's own source comments
flag the fragility directly: the calls must "stay consistent across both call
sites... otherwise rewound content can leak back into prompts through a
compaction summary" (`src/google/adk/apps/compaction.py:390-393,540-543`).

**Blast radius.** Additive -- a clarifying sentence in
[ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) facet 8
naming the context compiler as the sole entry point for both `Compacted` and
`SessionRewound.keep_through` bounds. No proto or schema change.

**Why.** Decision 8 already structurally avoids ADK's mistake by compiling the
model-visible context as one named projection rather than leaving every
consumer to write its own fold. But "already structurally avoids it" is an
architectural accident until the ADR says so explicitly; ADK's own comments
are direct evidence of what happens when a correctness-critical fold is a
convention rather than an enforced single path -- two call sites, written by
the same team, in the same file family, still needed a comment reminding
themselves to stay in sync. Our design should not rely on every future
projection author independently rediscovering that reminder.

**Cost.** None beyond the sentence; it becomes a real cost only if an
implementation audit later finds a projection that has already grown its own
ad hoc liveness check.

### 2. State explicitly that a derived read model must never gain an independent write path

**The change.** [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 8 calls the aggregate snapshot "an advisory
cached fold of that log," and decision 3 calls it one of "four records with
separate authority," but neither passage explicitly forecloses a future
implementer adding a direct-write fast path to a snapshot or projection under
performance pressure.

**Evidence anchor.** Google ADK, store maturity 10/12: `Session.state` is
exactly this shortcut, taken by every one of ADK's local backends --
`storage_session.state.update(...)`
(`src/google/adk/sessions/database_session_service.py:931-932`) and
`json_patch()`
(`src/google/adk/sessions/sqlite_session_service.py:552-596`) -- and "no
backend fold-replays `events` to reconstruct `state`... makes `events` the
only true append-only source of truth, and `state` a
rebuildable-in-principle but not-actually-rebuilt projection" (dossier, "The
storage model").

**Blast radius.** Additive -- a Non-Goal or explicit constraint statement in
decision 8; no schema change.

**Why.** This is already implied by decision 8's language, but ADK proves the
implication is not obvious enough to survive contact with a real
implementation: a widely distributed, actively maintained framework made
exactly this trade for its own load-bearing state, apparently without
anyone treating it as a violation of the store's own stated principles.
Naming the constraint explicitly, with ADK cited as the cautionary evidence,
turns "we wouldn't do that" into a documented rule a future PR under deadline
pressure has to argue against rather than quietly work around.

**Cost.** None now; real cost only if a future change actually needs the rule
enforced against a proposed shortcut.

### 3. Resolve whether a lightweight, non-`DispatchDelegation` subagent path is already fully covered

**The change.** Confirm, in the ADR, whether a single-turn subagent
invocation that does not need independent resumability or an independent audit
boundary is already fully expressible as an ordinary
`ToolCallRequested`/`ToolCallCompleted` pair with no `DispatchDelegation` at
all -- or whether decision 6 should name this explicitly as a second, cheaper
delegation shape alongside the full child-session model.

**Evidence anchor.** Google ADK, store maturity 10/12: its branch-scoped
subagent model has "no separate `Session` object, no separate store row, and
no parent-delete cascade concern because there is nothing separate to cascade
to" (dossier, "Subagents and nested sessions," citing
`src/google/adk/tools/agent_tool.py:385-398`,
`src/google/adk/agents/context.py:424-479`) -- a genuinely cheap answer for the
case where a subagent doesn't need its own durable identity. Its
*discouraged* throwaway `AgentTool` model shows the opposite outcome: "the
child session and its entire event transcript are never persisted anywhere
beyond that in-memory `Runner`'s lifetime... discarded as soon as the tool
call returns" (`src/google/adk/tools/agent_tool.py:225-310`), a path ADK's own
docstring calls out as discouraged (`agent_tool.py:125-126`).

**Blast radius.** Additive as a clarifying question and, if the answer is
"yes, ordinary tool calls already cover it," a documentation-only conclusion.
Breaking the decision only if the ADR later decides decision 6 needs a new,
officially sanctioned lightweight delegation shape distinct from both an
ordinary tool call and a full `DispatchDelegation`.

**Why.** This is not a gap to close by copying ADK's branch model; it is worth
resolving explicitly because ADK's own two-tier split shows both outcomes of
skipping child-session identity for a subagent: cheap and safe when the
subagent genuinely doesn't need independent durability (branch scoping), and
a silent, crash-invisible data-loss risk when it does (the discouraged
`AgentTool` path -- "a crash there is indistinguishable from a normal tool-call
failure from the parent session's point of view," per the dossier). Naming,
in the ADR, which invocations are expected to go through `DispatchDelegation`
versus an ordinary tool call closes an ambiguity ADK's own maintainers
apparently found easy to get wrong, since they had to write a docstring
discouraging their own worse tier rather than removing it.

**Cost.** None if the answer is "already covered" -- this recommendation's
whole value is foreclosing a future implementer from reinventing ADK's
discouraged pattern out of a mistaken belief that every subagent call needs
full delegation machinery.

### 4. Adopt an explicit schema-version marker and a written back-compat policy at the `v1alpha1` → `v1` promotion

**The change.** Already tracked as open in the
[fx comparison](../fx/vs-session-events.md#9-migrations-are-not-journaled),
item 9: record a schema migration as an auditable event, with a
version marker and digests either side, rather than leaving it an operational
memory.

**Evidence anchor.** Google ADK, store maturity 10/12, is corroborating
evidence from an entirely unrelated product: a real, shipped schema-version
row (`adk_internal_metadata`,
`src/google/adk/sessions/schemas/v1.py:55-67`), detect-then-branch sniffing
for databases predating that row
(`src/google/adk/sessions/migration/_schema_check_utils.py:70-89`), and a
written deprecation policy requiring "backward-compatible with the previous
schema for a few releases (at least 2)"
(`src/google/adk/sessions/migration/README.md:106-129`) before an old branch
is removed.

**Blast radius.** Breaking, cheap when it lands -- a new event type plus a
version field, no rewrite of existing events.

**Why.** ADK doesn't change the fx recommendation's reasoning, it strengthens
its urgency: this is now two independently built products (fx, ADK) that both
found it necessary to treat schema evolution as a first-class, auditable
operation rather than an implicit one, for the same reason -- a database or
log that silently contains two schema generations needs a way to tell them
apart that survives longer than institutional memory.

**Cost.** One message type plus the discipline of stamping it at the actual
`v1` cutover; no cost until that promotion happens.

### 5. State who must satisfy the `NoStream`/`At`/`Any` guarantee if a non-JetStream backend or tenant binding is ever introduced

**The change.** [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 2's per-command precondition classification
is a substrate-level guarantee today: "the runtime resolves the append guard
to `At(current_position))`... unless an aggregate opts out." Nothing in the
ADR states who is responsible for re-verifying that guarantee if a deployment
giving one tenant its own stream and bucket
([ADR#0027](../../../../adr/0027-decider-multi-tenancy-primitive.md)) or a
future storage tier is ever backed by something other than NATS JetStream.

**Evidence anchor.** Google ADK, store maturity 10/12: "optimistic concurrency
with an expected-version precondition exists only in `DatabaseSessionService`,
and is materially weaker in `SqliteSessionService`, and entirely absent in the
in-memory and Vertex backends. A store abstraction that is 'the same
interface' across these four backends hides a real behavioral cliff on
concurrent append" (dossier, "Write and append path"). `SqliteSessionService`'s
staleness check is a bare, untyped `ValueError` compared to
`DatabaseSessionService`'s marker-based check with a named constant
(`src/google/adk/sessions/sqlite_session_service.py:405-420` vs.
`src/google/adk/sessions/database_session_service.py:904-924`).

**Blast radius.** Additive -- a Non-Goal or explicit obligation statement; no
schema change today, since we have exactly one backend (NATS JetStream) and
decision 2's guarantee already holds for it by default.

**Why.** ADK's four backends satisfy the same method signatures
(`BaseSessionService`) while silently disagreeing about what happens under
concurrent append -- the interface promised more uniformity than it actually
delivered. Our situation is stronger today only because we have one substrate;
the moment a second one is entertained, the same risk ADK demonstrates
becomes live for us too, and nothing currently written down says whose job it
is to prove the new backend still enforces `At(current_position)` the way
JetStream does today.

**Cost.** None until a second backend is actually proposed; at that point, the
cost is a conformance test suite for the precondition contract, not a schema
change.

## What our design already does better

- **Real, substrate-level optimistic concurrency by default, not a
  per-backend afterthought.** [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 2: "When a command declares no
  `WRITE_PRECONDITION`, the runtime resolves the append guard to
  `At(current_position)`... this is already satisfied on this substrate for
  free, unless an aggregate opts out." ADK's equivalent exists in exactly one
  of four backends, is cruder in a sibling, and is entirely absent in two
  (dossier, "Write and append path").
- **Rewind needs no computed reversal payload.** ADK's rewind must walk
  forward and diff to compute a reversing `state_delta`
  (`_compute_state_delta_for_rewind`, `src/google/adk/runners.py:1380-1405`)
  because `state` is a folded document that has to be told how to un-happen.
  Our `SessionRewound.keep_through` (`session_rewound.proto`) needs no
  reversal computation at all: nothing is folded past the boundary, so there
  is nothing to reverse.
- **A designed idempotency contract, not an accidental one.** ADK's `Event.id`
  is a client-assigned UUID4 whose only protection against a retried append is
  an incidental primary-key collision (dossier, "Write and append path"). Our
  `Event.id` is deterministically derived from `(subject, command type,
  idempotency key, batch index)` ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 2) -- retries are safe by
  construction, not by accident.
- **Fold-derived ordinals, immune to clock skew.** ADK orders events by
  client-generated `Event.timestamp` with a UUID tiebreak on ties, which the
  database layer's own code comment explains exists only because "the
  database is free to return tied events in a different order on every read"
  (`src/google/adk/sessions/database_session_service.py:693-696`) -- a real
  admission that client clocks can reorder a replayed conversation. Our
  `SessionOrdinal` is fold-derived and never a physical or client-supplied
  value ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 2).
- **Delegation always buys a genuinely durable, resumable, auditable child.**
  ADK's discouraged `AgentTool` path can lose an entire subagent run to a
  crash with no trace at all -- "indistinguishable from a normal tool-call
  failure from the parent session's point of view" (dossier, "Subagents and
  nested sessions"). Every one of our delegations is `DelegationDispatched`
  before child creation, with crash-safe reconciler repair ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision
  6): there is no path in our design where a dispatched child can vanish
  without a trace.
- **One documented read-model contract, not a docstring three of four
  backends contradict.** ADK's `ListSessionsResponse` docstring claims "states
  are not set," which `InMemorySessionService`, `DatabaseSessionService`, and
  `SqliteSessionService` all violate (dossier, "The store interface"). Our
  `SessionProjection` ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 8) *is* the documented read contract;
  there is no separate prose description of behavior for an implementation to
  drift away from.
- **Redaction and erasure are named, typed facts.** `RedactionApplied` and
  `ArtifactErased` ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 7) have no analogue anywhere in the ADK
  dossier -- no redaction, masking, or byte-erasure concept was found in
  `src/google/adk/sessions/` at all.
- **Cascade policy is a recorded, typed fact per child, not an emergent
  property of storage choice.** `CascadePolicy` (`cascade_policy.proto`) makes
  "does this child survive its parent's terminal state" an explicit,
  per-delegation decision. ADK has no equivalent concept for either of its
  in-session subagent models, and its throwaway model has no cascade story
  because it has no durable child to cascade to.

## Trade-offs, not gaps

- **Branch-scoped, same-stream subagents vs. always-separate child streams.**
  ADK's branch model (`Event.branch`, a dot-separated ancestor path filtering
  one shared event list) costs nothing extra per subagent invocation: no new
  stream, no dispatch saga, no cascade reconciliation. Ours always pays for a
  full `DelegationDispatched`/`ParentLinked` saga ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 6), even
  for a subagent call that never needs independent resumability. ADK's
  approach buys cheapness at the cost of no independent identity -- a sibling
  agent's history must be manually hidden by string-prefix filtering, and
  nothing about the branch is independently resumable, auditable, or
  redactable on its own terms. Ours buys the opposite: every delegation,
  however small, is a first-class citizen with its own durability and
  redaction story, at the cost of dispatch/cascade machinery for every one.
  Recommendation 3 above exists to make this an explicit choice rather than an
  unexamined default in either direction.
- **Schema-version branching vs. additive-only evolution.** ADK's
  `adk_internal_metadata` plus parallel v0/v1 model classes let it express a
  genuinely breaking payload change (Python pickle to JSON) without minting a
  new event type, at the cost of maintaining two full parallel schema
  generations in the codebase for "at least 2" releases
  (`src/google/adk/sessions/migration/README.md:123-129`). [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 3
  is additive-only, "never a per-event version branch" -- we cannot express
  that kind of breaking change without a new event type, but we never carry
  two live schema generations of the same event type at once.
- **State merged at every read vs. context compiled at every read.** ADK's
  `_merge_state` folds session, `app:`, and `user:` scoped state together on
  every `get_session` call (`src/google/adk/sessions/database_session_service.py:245-256`)
  -- cheap, because it is a merge over a handful of already-mutable buckets,
  never a fold over history. Our model-visible context is compiled from the
  event log bounded by the latest snapshot/`Compacted` marker on every read
  too ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 8) -- potentially more expensive per read, but it is
  the only way to guarantee the result cannot silently disagree with the log,
  which is exactly the guarantee ADK's `state` column does not have.

## What not to copy

- **`Session.state` as a directly-mutated document with no fold-check against
  `events`.** The direct antithesis of [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 8. Nothing in ADK's
  four backends ever compares `state` against what replaying `events` would
  produce, so the two can silently disagree with no detection mechanism at
  all.
- **The throwaway `AgentTool` subagent path.** A crash mid-call discards the
  entire child transcript with no trace, "indistinguishable from a normal
  tool-call failure" -- and ADK's own maintainers discourage it in their own
  docstring rather than removing it. This is the single clearest cautionary
  example in this dossier of what happens when a delegation is not given
  durable identity before it starts running.
- **A correctness-critical fold relied on by convention across independent
  call sites.** `_apply_rewinds` needing a source comment reminding two call
  sites to stay in sync is a real, self-diagnosed fragility, even though the
  underlying rewind-as-appended-marker shape is sound and matches ours.
  Recommendation 1 exists specifically to avoid inheriting this pattern.
- **A documented read contract that the implementation silently
  contradicts.** `ListSessionsResponse`'s docstring claims "states are not
  set," which three of four backends violate. This echoes the fx
  comparison's "underversioned public projection" lesson: our
  `SessionProjection` read contract must never be allowed to drift from a
  written description of it the way this docstring did.
- **A compound identity key that bakes in scoping.** `(app_name, user_id,
  session_id)` as a literal composite primary key is workable for ADK, but it
  is exactly the coupling we are deliberately avoiding by keeping `SessionId`
  opaque and leaving tenant/scope binding to the resolver's declared subject
  scope ([ADR#0027](../../../../adr/0027-decider-multi-tenancy-primitive.md))
  rather than baking it into identity.

## The two gaps the industry has not closed

### Subagent cascade

[ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 6 already takes a position: a child session is its own
logical stream, linked by facts on each side (`DelegationDispatched`,
`ParentLinked`), dispatch is parent-first with crash-safe reconciler repair,
rewind invalidation is distinct from terminal cascade, and terminal cascade is
"transitive by construction" through a reconciler reacting to its own emitted
terminal markers. The question here is whether ADK's evidence validates,
challenges, or refines that position, not whether we still need one.

**What ADK does when a parent is deleted, rewound, or crashes with a live
child.** ADK ships three subagent storage shapes with two distinct durability
fates. Branch-scoped subagents (`sub_agents=[...]`, single-turn `AgentTool`
usage) and `isolation_scope`-filtered Task-API delegates both write into the
*same* `Session.events` list as the parent, tagged only by a longer `branch`
path (`src/google/adk/tools/agent_tool.py:385-398`) or a narrower
`isolation_scope` filter (`src/google/adk/events/event.py:136-149`) -- there is
no separate `Session` object for either. The fully separate, discouraged
multi-turn `AgentTool` path constructs a brand-new `Runner` with a brand-new
`InMemorySessionService()` per call
(`src/google/adk/tools/agent_tool.py:225-271`) and never persists that child
session anywhere beyond the call's own lifetime.

- **On parent delete:** the two in-session models have "no separate store row,
  and no parent-delete cascade concern because there is nothing separate to
  cascade to" (dossier). The throwaway model has nothing to cascade either,
  because nothing was ever durable.
- **On parent rewind:** `_apply_rewinds` naturally covers branch-tagged
  subagent events, since they live in the same list as everything else being
  rewound; no separate child-rewind-cascade concept exists. The throwaway
  model is out of scope of rewind entirely, since its events never entered the
  durable store.
- **On crash mid-subagent-call:** the in-session models leave whatever partial
  branch-tagged events had already been appended, consistent with ADK's normal
  per-event durability story. The throwaway model loses the entire child run --
  "a crash there is indistinguishable from a normal tool-call failure from
  the parent session's point of view" (dossier, "Subagents and nested
  sessions").

**Does this validate, challenge, or refine decision 6?** It validates the
core design on both of its load-bearing claims, and sharpens one thing
decision 6's text does not yet name. ADK's cheap, in-session branch model
proves the industry does have one legitimate way to sidestep cascade
complexity entirely: give the subagent no independent identity at all, so
there is nothing separate to invalidate on rewind or cascade on delete. That
is a real option decision 6 does not currently offer -- every one of our
delegations always gets a full child stream, dispatch saga, and cascade
eligibility, however small the subagent invocation. Recommendation 3 above
exists to make this an explicit choice rather than something the ADR is
silent on. Separately, ADK's *discouraged* throwaway model is the clearest
possible validation of why decision 6 insists a child gets durable identity
*before* it starts running: `DelegationDispatched` lands on the parent's
stream before child creation, and a reconciler repairs a missing child from
that fact alone if a crash happens in between ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 6). ADK's
throwaway path has no equivalent durable dispatch fact at all -- the entire
delegation is invisible to the store until the tool call returns
successfully, which is precisely the failure decision 6's parent-first
dispatch is built to prevent, and ADK's own maintainers know it well enough to
discourage the pattern in their own docstring without removing it.

### Retention on an unbounded log

[ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 7 already takes a position: keep-forever, with
`SessionHidden` as a visibility tombstone, `RedactionApplied` for read-time
masking, `ArtifactErased` for out-of-band artifact-byte destruction, and
aggregate snapshots bounding replay cost rather than storage size. The
question is whether ADK's evidence validates that design or exposes a cost
the ADR does not currently bound.

**What ADK does.** No TTL or scheduled cleanup exists in
`InMemorySessionService`, `DatabaseSessionService`, or `SqliteSessionService`;
`delete_session` is the only removal path and "nothing calls it automatically
anywhere searched in `src/google/adk/sessions/` or `src/google/adk/runners.py`"
(dossier, "Retention, deletion, and multi-host"). `VertexAiSessionService`
alone accepts a caller-supplied `ttl`/`expire_time`
(`src/google/adk/sessions/vertex_ai_session_service.py:179-200`), delegating
retention entirely to the remote, paid Agent Engine service rather than
answering it at the store layer. `app_states`/`user_states` rows are never
deleted by `delete_session` regardless of how many sessions referenced them --
"an intentional consequence of the scoping model... but a real
orphan-accumulation risk with no visible cleanup path in this package"
(dossier, "Retention, deletion, and multi-host"). `list_sessions` has no
pagination parameters at all, so listing cost is "whatever a full per-app
(optionally per-user) table scan costs on the underlying engine" (dossier,
"Listing, summaries, and search"). No issue-tracker report of an actual
user-visible growth failure was found anywhere in the dossier -- this is
notably thinner evidence than, for example, the Cline comparison's
corroborated `cline/cline#9011` growth failure, and should be read as
inference from design shape, not a confirmed field failure.

**Does this validate, challenge, or refine decision 7?** It validates the
core position that retention is not solved by any particular storage shape and
must be designed deliberately, and it refines decision 7 in one place its
text does not currently cover. ADK is a third independent data point
(alongside the two purest event-sourced products the original synthesis
names) showing that "no retention story at all" is not specific to pure
event-sourcing -- a mixed document-plus-log store has exactly the same gap.
That is direct support for decision 7's premise that nothing in any of these
patterns forces retention design; it has to be a deliberate decision, which is
what decision 7 already is. The refinement is `app_states`/`user_states`:
decision 7's redaction/hide/erase contract is scoped to one session's own
stream, and we have no equivalent to ADK's cross-session, app-scoped or
user-scoped shared state in `v1alpha1` today -- so this specific orphan risk
does not currently apply to us. But if such a feature is ever added, decision
7 would need an answer for a value with no single owning stream to redact,
hide, or erase, and ADK's own such state is direct evidence of what the
unaddressed version of that problem looks like: rows that outlive every
session that ever wrote them, with "no visible cleanup path" found anywhere in
the package. This is worth an explicit open question rather than assuming it
away by the absence of the feature today.

## Open questions for the ADR

1. Should [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md)
   facet 8 name the model-visible context compiler as the sole mandatory fold
   point for both `Compacted` and `SessionRewound.keep_through` bounds,
   foreclosing an independent ad hoc liveness check the way ADK's
   `_apply_rewinds` needed two call sites to agree on its own?
2. Should decision 8 explicitly forbid an aggregate snapshot or any derived
   read model from ever gaining a direct write path independent of `evolve`,
   given that ADK's `state` column is exactly that shortcut and nothing in
   ADK checks it against `events`?
3. Is a lightweight, non-`DispatchDelegation` path for a single-turn subagent
   invocation that does not need independent resumability already fully
   covered by an ordinary `ToolCallRequested`/`ToolCallCompleted` pair, or
   does decision 6 want to name this explicitly as the sanctioned answer to
   ADK's branch-scoped model?
4. If a future storage backend or per-tenant stream
   ([ADR#0027](../../../../adr/0027-decider-multi-tenancy-primitive.md)) is
   ever backed by something other than NATS JetStream, who is responsible for
   verifying it independently satisfies the same `NoStream`/`At`/`Any`
   guarantee facet 2 assumes today, given ADK shows a shared interface can
   silently vary concurrency semantics per backend?
5. When `v1alpha1` promotes to `v1`, should the promotion include an explicit
   schema-version marker and a written backward-compatibility policy (as fx's
   comparison already proposes and ADK's `adk_internal_metadata` plus
   migration README corroborate), rather than relying solely on additive
   evolution?
6. Do we ever want a cross-session, app-scoped or user-scoped durable value
   analogous to ADK's `app:`/`user:` prefixed state? If so, what stream owns
   redacting, hiding, or erasing it under decision 7's contract, given ADK's
   own such state has no owning session and "no visible cleanup path" once
   every session that ever referenced it is gone?

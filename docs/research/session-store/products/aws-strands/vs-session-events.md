# AWS Strands Agents compared to our session event catalog

Part of Session Store Research.
Produced by running [RESEARCH_PROMPT_COMPARISON](../../RESEARCH_PROMPT_COMPARISON.md).
Stage-one dossier: [AWS Strands Agents](./index.md).
Compared against `proto/trogonai/session/sessions/v1alpha1/` and ADR#0035 on 2026-08-04.

**Store maturity: 6/12** -- evolution scars 0/3 (no `schema_version` field, no
migration function, no legacy-format sniffing anywhere in `session/` or
`types/session.py`, per
the dossier's [Schema evolution: no version field on any durable session type](./index.md#schema-evolution-no-version-field-on-any-durable-session-type) section;
the two `schema_version` fields in the whole persistence surface belong to
adjacent, explicitly non-store mechanisms, `Checkpoint` and `Snapshot`
(the dossier's [Schema evolution: no version field on any durable session type](./index.md#schema-evolution-no-version-field-on-any-durable-session-type) section), and the team's own
`team/designs/0014-storage.md` proposes replacing the whole per-subsystem
interface rather than evolving it in place
(the dossier's [What this implies for our Session Store (our inference)](./index.md#what-this-implies-for-our-session-store-our-inference) section)), operational age 1/3
(`_fix_broken_tool_use` is a real defensive read-path repair for corrupted
histories, referencing GitHub issue `strands-agents/harness-sdk#859` in the
code comment at
`strands-py/src/strands/session/repository_session_manager.py:240,245,405`,
and the dossier's [Read and resume path](./index.md#read-and-resume-path)
section places it on the restore path), but the dossier surfaces no
dated, field-confirmed corruption or growth incident the way Cline's
`cline/cline#9011` was confirmed with open/close dates), exposure 2/3
(AWS-branded, `authors = [{name = "AWS", email = "opensource@amazon.com"}]`
(`strands-py/pyproject.toml:14-16`), Apache-2.0 (`LICENSE.APACHE`), with
parallel Python and TypeScript SDKs (`strands-py/`, `strands-ts/`) and a
multi-host-capable S3 backend as a shipped option (the dossier's
[Retention, deletion, and multi-host](./index.md#retention-deletion-and-multi-host)
section), but no adoption-scale evidence, no plugin
ecosystem, and only one referenced issue), design independence 3/3 (no
evidence in the dossier that this store was forked from another product's
persistence code; both backends are original to this SDK). At 6/12 this sits
on the edge of the "thin evidence" threshold rather than below it, so its
recommendations are weighted as one data point, not an industry norm, and are
called out as such below.

## The one structural difference everything else follows from

Strands' store has no append primitive at all, on either backend. "Appending
a message" means creating a brand-new whole JSON object (a file or an S3 key)
per message, positionally keyed by a client-computed integer (`message_id`)
that doubles as the storage address
(the dossier's [The storage model](./index.md#the-storage-model) and [Keying and identity](./index.md#keying-and-identity) sections). This is not a granularity choice the
way fx's turn-level commit or Cline's whole-document rewrite are; it is the
direct consequence of choosing an object store as the durable substrate. A
backend that has no append operation must decompose everything into a set of
small, independently addressable writes and reconstruct order from the keys
themselves, and the dossier is explicit that this is deliberate: "the store
never needs a true 'append' primitive, which sidesteps the one operation an
object store cannot do natively" (the dossier's [The storage model](./index.md#the-storage-model) section).

Nothing about this store's mutation model resembles ours. Our design commits
at fact granularity too, but every commuting fact still goes through a real
append onto a shared logical stream, and every invariant-bearing fact is
guarded by a server-enforced `WRITE_PRECONDITION`
(`docs/adr/0035-session-store-decider-aggregate.md:181-192`). Strands has no
server-enforced guard anywhere, on either backend, for creation, for
positional ordering, or for deletion: "neither backend has any locking, or
compare-and-swap / conditional-write precondition on any operation. No file
locks, no S3 `If-Match`/`If-None-Match` conditional headers, no version/ETag
checks anywhere" (the dossier's [Write and append path](./index.md#write-and-append-path-ordering-durability-concurrency-delivery) section). That absence is not an
oversight parallel to ours being incomplete; it is coherent with the rest of
the design, because a fresh, uniquely-keyed whole-object write never needs a
guard against a concurrent editor the way an append or an in-place update
would -- unless two writers pick the *same* key, which nothing in either
backend prevents.

Everything else documented in the dossier is a consequence of that one
missing primitive, not an independent design choice: the racy `create_session`
check-then-act on both backends (the dossier's [Write and append path](./index.md#write-and-append-path-ordering-durability-concurrency-delivery) section), the racy
`message_id` counter collision when two managers resume the same session
independently (the dossier's [Write and append path](./index.md#write-and-append-path-ordering-durability-concurrency-delivery) section), whole-session-only deletion
diverging in atomicity between the two backends
(the dossier's [Retention, deletion, and multi-host](./index.md#retention-deletion-and-multi-host) section), and the total absence of a version field on
any durable session type (the dossier's [Schema evolution: no version field on any durable session type](./index.md#schema-evolution-no-version-field-on-any-durable-session-type) section) all trace back to a
store that was shaped, correctly, to avoid needing a write-ordering primitive
at all -- and then never added one back for the operations (creation,
positional counters, deletion) that still needed one.

## Mapping

| Strands | Ours | Verdict |
| --- | --- | --- |
| `Session{session_id, session_type, created_at, updated_at}`, `SessionType` a `str, Enum` with one member `AGENT` (dossier: [Entry and message structure and versioning](./index.md#entry-and-message-structure-and-versioning)) | No `session_type` field; a session's kind is implicit in its `StoredSessionExecutionPlan` | Gap, minor -- deliberate on both sides: Strands' enum anticipates growth it hasn't needed yet, ours never modeled the concept |
| `session_id`/`agent_id`, caller-supplied strings, path-separator-validated, used verbatim as directory names/key prefixes (dossier: [Keying and identity](./index.md#keying-and-identity)) | `SessionId`, opaque, resolved through a `StreamSubjectResolver` to `session.sessions.events.<session_id>`; a subject-token-unsafe id is mapped through a routing-key transform (`docs/adr/0035-session-store-decider-aggregate.md:98-106`) | Semantic mismatch -- Strands' id *is* the storage address; ours is never itself a filesystem or subject token |
| `message_id`, integer position, client-computed in `RepositorySessionManager`'s process memory, doubles as the storage key: `_get_message_path` interpolates it straight into `message_<id>.json` and rejects a non-integer (`strands-py/src/strands/session/file_session_manager.py:100-117`) | `SessionOrdinal`, fold-derived, "derived by counting at fold time, never read from JetStream message metadata... never a self-position naming" (`proto/trogonai/session/sessions/v1alpha1/session_ordinal.proto:5-15`, `docs/adr/0035-session-store-decider-aggregate.md:140-168`) | Ours, decisively -- see structural difference above |
| `tracking_id`, durable UUID content identity, recorded but never read or checked by either backend for dedup (dossier: [Write and append path](./index.md#write-and-append-path-ordering-durability-concurrency-delivery)) | `CanonicalMessage.message_id`, stable message id, the join key for first-terminal-outcome-wins fold on `AssistantMessageCompleted`/`AssistantMessageFailed` (`proto/trogonai/session/sessions/v1alpha1/message.proto:14-28`, `docs/adr/0035-session-store-decider-aggregate.md:200-207`) | Ours, decisively -- the same field exists on both sides, but only ours is load-bearing for anything |
| `SessionAgent.state`/`_internal_state{interrupt_state, model_state}`, a durable JSON document that *is* the authoritative resume path: `initialize_internal_state` assigns `agent._interrupt_state` and `agent._model_state` straight out of it (`strands-py/src/strands/types/session.py:176-181`), called from the restore path at `strands-py/src/strands/session/repository_session_manager.py:209` | Aggregate snapshot, "an advisory cached fold of that log. Corruption or incompatibility falls back to earlier replay" (`docs/adr/0035-session-store-decider-aggregate.md:414-415`); harness recovery checkpoint, "an opaque artifact... It cannot replace event replay" (`:416-418`) | Semantic mismatch -- same problem (resume needs process state), opposite authority model: Strands' document is load-bearing, ours is explicitly disposable |
| `SessionAgent.conversation_manager_state{removed_message_count, optional summary}` (dossier: [Compaction and history management](./index.md#compaction-and-history-management)) | `Compacted{covers_from, covers_through, summary_content, tokens_before, tokens_after, model, usage}` (`proto/trogonai/session/sessions/v1alpha1/compacted.proto:19-38`) | Ours, decisively -- a range-addressed, self-sufficient marker vs. a bare offset counter with no boundary provenance |
| `SessionMessage.redact_message`, a second field written alongside the original on the *same* durable object, read-preferred by `to_message()` (dossier: [Entry and message structure and versioning](./index.md#entry-and-message-structure-and-versioning) and [Rewind, checkpoints, and fork](./index.md#rewind-checkpoints-and-fork)) | `RedactionApplied{redacted_event_ids, reason}`, a new event masking the targeted events at read time; original bytes never touched (`proto/trogonai/session/sessions/v1alpha1/redaction_applied.proto:5-19`) | Semantic mismatch, not equivalence -- same intent, opposite mechanism: Strands' redaction is the one in-place edit that exists anywhere in the system; ours performs zero edits, ever |
| `delete_session`: file backend `shutil.rmtree`; S3 backend paginated `delete_objects`, no resume marker (dossier: [Retention, deletion, and multi-host](./index.md#retention-deletion-and-multi-host)) | `SessionHidden` (visibility tombstone, no bytes deleted) + `ArtifactErased` (per-artifact, out-of-band byte destruction) + deferred crypto-shredding follow-up ADR (`proto/trogonai/session/sessions/v1alpha1/session_hidden.proto:5-16`, `.../artifact_erased.proto:5-17`, `docs/adr/0035-session-store-decider-aggregate.md:896-900`) | Semantic mismatch, and its own section below -- Strands attempts real erasure and can silently half-fail; we don't attempt erasure in `v1alpha1` at all, so we can't fail the same way, but we also haven't solved what Strands is trying to solve |
| No `list_sessions`/`list_agents` anywhere in `SessionManager` or `SessionRepository` (dossier: [Keying and identity](./index.md#keying-and-identity) and [Listing, summaries, and search](./index.md#listing-summaries-and-search)) | `get_session`, `list_sessions` as rebuildable KV projection queries (`docs/adr/0035-session-store-decider-aggregate.md:930-935`) | Ours, decisively |
| `Graph`/`Swarm` forbid a node's own `Agent` from holding a session manager at all; only the orchestrator's single `multi_agent.json` is durable, embedding only the *last* message per node via `AgentResult.to_dict()` (dossier: [Subagents and nested sessions](./index.md#subagents-and-nested-sessions)) | `DelegationDispatched`/`ParentLinked`/`CascadePolicy` give a child its own full durable session and stream (`proto/trogonai/session/sessions/v1alpha1/delegation_dispatched.proto:20-25`, `.../parent_linked.proto:19-27`, `docs/adr/0035-session-store-decider-aggregate.md:729-756`) | Ours, decisively -- see Subagent cascade below |
| No TTL/lifecycle/cleanup found anywhere; store retains everything until an explicit `delete_session` call (dossier: [Retention, deletion, and multi-host](./index.md#retention-deletion-and-multi-host)) | Keep-forever by design (`docs/adr/0035-session-store-decider-aggregate.md:857-863`), with `SessionHidden`/`RedactionApplied`/`ArtifactErased` as the explicit privacy contract plus optional reversible cold-tiering (`:911-921`) | Trade-off, not a plain win -- see Retention below |
| `Checkpoint.schema_version`/`Snapshot.schema_version` (both always `"1.0"`, hard-reject on mismatch), on two mechanisms the module docstrings say explicitly are *not* the session store (dossier: [Schema evolution: no version field on any durable session type](./index.md#schema-evolution-no-version-field-on-any-durable-session-type)); no version field on `Session`/`SessionAgent`/`SessionMessage` | No `schema_version` field on any session event either; "Schema evolution is additive (new optional fields, reserved retired numbers), never a per-event version branch" (`docs/adr/0035-session-store-decider-aggregate.md:378-379`) | Same absence, different mechanism underneath -- see recommendation 1 |
| `Checkpoint` (`experimental/checkpoint/checkpoint.py`): a mid-cycle pause marker, tool-cycle-only, explicitly paired with, not a replacement for, the session store (dossier: [Rewind, checkpoints, and fork](./index.md#rewind-checkpoints-and-fork)) | `Checkpoint` embedded in `CheckpointProduced`/`ExecutionAttemptStarted.restored_checkpoint`: attempt-scoped evidence with its own admission contract (digest, plan-digest equality, first-evidence-wins) (`proto/trogonai/session/sessions/v1alpha1/checkpoint.proto:8-38`, `docs/adr/0035-session-store-decider-aggregate.md:426-465`) | Semantic mismatch by name only -- both are called "Checkpoint" and both say "not a replacement for the transcript," but the two are not a like-for-like: Strands' fires only on tool-use cycles, ours is per-attempt and digest-verified |
| `Snapshot` (`types/_snapshot.py`, `agent.take_snapshot`/`load_snapshot`): opt-in, in-memory, versioned copy-plus-restore, entirely outside `SessionManager`/`SessionRepository`, no lineage metadata anywhere (dossier: [Rewind, checkpoints, and fork](./index.md#rewind-checkpoints-and-fork)) | `SessionForked`, an atomic `[SessionStarted, SessionForked]` in-stream creation batch recording `source_session_id` and `context_prefix_boundary` (`proto/trogonai/session/sessions/v1alpha1/session_forked.proto:7-27`, `docs/adr/0035-session-store-decider-aggregate.md:669-721`) | Ours, decisively -- fork is a first-class, in-band, lineage-recording domain event; Strands' nearest analogue is entirely out-of-band and unversioned in the store |
| `multi_agents/multi_agent_<id>/multi_agent.json`, a single whole-blob orchestrator state keyed by `node_results` (dossier: [Subagents and nested sessions](./index.md#subagents-and-nested-sessions)) | No second denormalized blob; the parent-child graph folds from `DelegationDispatched`/`ParentLinked` facts across streams, and the audit trail folds from the child's own message/tool events | Ours, decisively -- no second copy of "what a node did" that can drift from the node's own real session |
| Command exit status not distinguished from any other tool outcome anywhere in the dossier | `CommandTermination{exit_code \| signal}` on `ToolCallCompleted.termination`, deliberately kept out of the provider-visible result (`proto/trogonai/session/sessions/v1alpha1/command_termination.proto:5-22`, `.../tool_call_completed.proto:16-35`) | Ours, decisively -- also independently confirmed closed by fx's item 2 |
| Required `WorkspaceRef`-equivalent: none. `Session` carries only `session_id`, `session_type`, `created_at`, `updated_at`; no cwd/origin field of any kind (dossier: [Keying and identity](./index.md#keying-and-identity)) | `SessionStarted.workspace`, a required `WorkspaceRef{workspace_id, uri, revision}` (`proto/trogonai/session/sessions/v1alpha1/session_started.proto:16-24`, `.../workspace.proto:13-22`) | Ours, decisively |

## What we should consider changing

Ordered most-consequential first. Given the 6/12 maturity score, none of these
is presented as an industry norm; each stands on Strands' own evidence alone.

### 1. Name explicitly why the durable event catalog carries no `schema_version` field, and what happens the day a change cannot be additive

**The change.** ADR#0035 facet 3 states "Schema evolution is additive (new
optional fields, reserved retired numbers), never a per-event version branch"
(`docs/adr/0035-session-store-decider-aggregate.md:378-379`), but does not say
what the mechanism is the day an existing event's shape genuinely cannot
change additively.

**Evidence anchor.** Strands, store maturity 6/12: `Session`, `SessionAgent`,
and `SessionMessage` carry no `schema_version` field, and `from_dict` on all
three "silently drops unknown keys and defaults missing ones"
(`strands-py/src/strands/types/session.py:96-100,166-170,204-207`, per
the dossier's [Schema evolution: no version field on any durable session type](./index.md#schema-evolution-no-version-field-on-any-durable-session-type) section) -- while two adjacent, explicitly
*non*-durable mechanisms in the same codebase, `Checkpoint.schema_version` and
`Snapshot.schema_version`, both hard-reject a version mismatch
(the dossier's [Schema evolution: no version field on any durable session type](./index.md#schema-evolution-no-version-field-on-any-durable-session-type) section).

**Blast radius.** Additive -- a documentation clarification to facet 3, not a
schema change. I am not recommending a literal `schema_version` field be
added over facet 3's stated prohibition on per-event version branches; see
Why.

**Why.** The naive fix -- copy Strands' `Checkpoint`/`Snapshot` pattern onto
session events -- would directly contradict facet 3's stated principle, so
recommending it outright would be recommending a decision reversal, not a
refinement. But Strands' actual failure mode is not "no version field," it is
"an incompatible shape change is undetectable and gets silently absorbed,"
and our mechanism differs in kind, not just in degree: protobuf's structural
required-field presence, additive-only wire evolution, and the typed-decode-
and-reject boundary of decision 3 already reject a malformed or incompatible
payload loudly, where Strands' Python dataclass field-filtering absorbs it
silently. The version field itself is not the fix; the fix is already in
place. What remains genuinely open, and what Strands' internal inconsistency
is useful evidence for, is unstated: the ADR should say explicitly what the
concrete mechanism is for a truly non-additive change (a new event type added
to the oneof, a reserved-and-replaced field number, something else), so a
future implementer facing that day does not reach for Strands' pattern (a
field that silently tolerates drift) as the path of least resistance under
time pressure.

**Cost.** None beyond the documentation; becomes a real cost only when a
genuinely non-additive change is actually needed and the mechanism has to be
invented on the spot instead of decided in advance.

### 2. When the deferred erasure-grade-deletion follow-up ADR is written, model it as N atomic per-item facts, never a bulk destroy

**The change.** ADR#0035 facet 7 defers "legal or user-requested erasure
beyond masking -- per-session encryption and key destruction... to a named
follow-up ADR"
(`docs/adr/0035-session-store-decider-aggregate.md:896-900`). This
recommendation is about the shape that follow-up should take, not a change to
`v1alpha1` today.

**Evidence anchor.** Strands, store maturity 6/12: the S3 `delete_session`
path pages through `list_objects_v2` and issues `delete_objects` in batches of
up to 1000 keys in a loop with no recorded resume point
(`strands-py/src/strands/session/s3_session_manager.py:191-212`, per
the dossier's [Retention, deletion, and multi-host](./index.md#retention-deletion-and-multi-host) section); the dossier is explicit that "if the process
crashes or a batch call raises partway through the loop, the session is left
with some keys deleted and others present, with no recorded resume point or
partial-delete marker anywhere in the code."

**Blast radius.** Additive -- `ArtifactErased` already is a per-artifact,
`At`-guarded, one-fact-per-item event
(`proto/trogonai/session/sessions/v1alpha1/artifact_erased.proto:5-17`). The
recommendation is to extend that shape (for example, a per-item crypto-
shred-completed fact) when the follow-up ADR is written, rather than to
introduce a bulk multi-key destroy operation.

**Why.** Strands' S3 backend is exactly the negative case our per-item
pattern already avoids: because every `ArtifactErased` is its own
`At`-guarded event on the log, a partial failure across many artifacts leaves
an exact, queryable record of which ones succeeded and which didn't -- a fold
over events already appended -- the opposite of Strands' silent gap, where
nothing in the code path can tell "fully erased" from "half erased" after a
crash. This is worth stating before the follow-up ADR is drafted, so a future
author reaching for a batch API, the obvious and more performant shape for
"erase everything about this session," does not reintroduce the exact failure
mode this dossier documents.

**Cost.** A slower erasure operation for a session with many artifacts (N
appends instead of one bulk call), which is the direct trade for a
resumable, auditable partial-failure state.

### 3. State explicitly whether cold-tier relocation to the JetStream Object Store needs any write precondition of its own

**The change.** ADR#0035 facet 7 permits, as an optional deployment choice,
copying "already-immutable old events... to the JetStream Object Store,
evicted from the hot stream, and restored on demand"
(`docs/adr/0035-session-store-decider-aggregate.md:915-921`), but does not
state whether that relocation needs a write-precondition of its own, given
that JetStream's Object Store, like S3, is not append-native.

**Evidence anchor.** Strands, store maturity 6/12: the corpus's clearest
demonstration of what "object store, zero added concurrency discipline"
produces is two silent races the dossier confirms are identically shaped on
both backends -- `create_session`'s check-then-act
(`strands-py/src/strands/session/file_session_manager.py:164-180`,
`strands-py/src/strands/session/s3_session_manager.py:166-181`, per
the dossier's [Write and append path](./index.md#write-and-append-path-ordering-durability-concurrency-delivery) section) and the `message_id` counter collision across
two independently resuming managers
(`strands-py/src/strands/session/repository_session_manager.py:69-86`, per
the dossier's [Write and append path](./index.md#write-and-append-path-ordering-durability-concurrency-delivery) section).

**Blast radius.** Additive -- a documentation clarification to facet 7's
Consequences/Non-Goals. If the audit this recommendation asks for turns up an
actual gap, the fix (a conditional-put on the tiering job) is Breaking,
cheap -- an implementation detail, no event shape changes.

**Why.** Our tiering job is architecturally unlike Strands' scenario: it is a
single promotion process moving already-committed, already-ordered, immutable
hot-stream bytes, never two independent writers targeting the same tiered
key the way two Strands processes can independently resume the same
`session_id`. Strands' two races likely don't transfer to our tiering story --
but "likely doesn't transfer" is exactly the kind of unstated assumption
Strands' own races show is worth writing down rather than assuming, since
both of Strands' races are silent (discovered only by reading the code, never
by a test failing) and cost nothing to rule out explicitly in advance.

**Cost.** None beyond writing the sentence, unless the audit surfaces a real
gap in the tiering job's design.

## What our design already does better

- **Position identity is fold-derived and is never a storage key.**
  `SessionOrdinal` is "derived by counting at fold time, never read from
  JetStream message metadata" and "never writes its own predicted position
  into its payload"
  (`proto/trogonai/session/sessions/v1alpha1/session_ordinal.proto:5-15`,
  `docs/adr/0035-session-store-decider-aggregate.md:140-168`). Strands'
  `message_id` is exactly the opposite: client-computed in process memory and
  used as the literal filename/key, which is precisely how two independent
  managers resuming the same session collide on the same key with different
  content (the dossier's [Write and append path](./index.md#write-and-append-path-ordering-durability-concurrency-delivery) section).
- **A real, server-enforced concurrency guard on invariant-bearing writes.**
  `At(current_position)` rejects a stale writer at the broker for every
  lifecycle and ledger transition
  (`docs/adr/0035-session-store-decider-aggregate.md:186-192`). Strands has no
  concurrency control anywhere on either backend -- "no file locks, no S3
  `If-Match`/`If-None-Match` conditional headers, no version/ETag checks"
  (the dossier's [Write and append path](./index.md#write-and-append-path-ordering-durability-concurrency-delivery) section) -- so every write in Strands is the equivalent of our
  `Any` bucket, with none of the invariants our `At`-guarded commands enforce
  (one active attempt, mutually exclusive approve/deny, one terminal outcome
  per operation) expressible at all.
- **Listing and search are first-class rebuildable projections.** `get_session`
  and `list_sessions` fold from the log
  (`docs/adr/0035-session-store-decider-aggregate.md:930-935`). Strands has no
  listing operation anywhere in `SessionManager` or `SessionRepository`; a
  caller must already know the `session_id` to address anything
  (the dossier's [Keying and identity](./index.md#keying-and-identity) and [Listing, summaries, and search](./index.md#listing-summaries-and-search) sections).
- **Redaction never mutates a stored record.** `RedactionApplied` is a new
  append naming event ids to mask at read time, automatically covering every
  duplicate (shared deterministic event id) and every fork's inherited
  context (read-by-reference)
  (`proto/trogonai/session/sessions/v1alpha1/redaction_applied.proto:5-19`,
  `docs/adr/0035-session-store-decider-aggregate.md:872-882`). Strands'
  `redact_message` is a second field written onto the *same* durable object
  in place -- the one edit that exists anywhere in this store -- and has no
  propagation story at all, because Strands has no fork/inheritance-by-
  reference concept to worry about in the first place.
- **Fork is a first-class, atomic, lineage-recording domain event.**
  `SessionForked` records `source_session_id` and `context_prefix_boundary` in
  the child's own creation batch
  (`proto/trogonai/session/sessions/v1alpha1/session_forked.proto:7-27`).
  Strands' nearest analogue, `Snapshot`, is "entirely outside
  `SessionManager`/`SessionRepository`... there is no lineage metadata
  recorded anywhere" (the dossier's [Rewind, checkpoints, and fork](./index.md#rewind-checkpoints-and-fork) section), and persisting it durably is
  entirely the caller's problem.
- **A delegated child gets the same durability, resume, audit, and redaction
  machinery as any other session.** `DelegationDispatched`/`ParentLinked`
  make a child a real, independently resumable session
  (`docs/adr/0035-session-store-decider-aggregate.md:729-756`). Strands
  forbids a node's own `Agent` from holding a session manager at all
  (the dossier's [Subagents and nested sessions](./index.md#subagents-and-nested-sessions) section) -- see Subagent cascade below.
- **A required, recorded workspace binding.** `SessionStarted.workspace` is a
  required `WorkspaceRef`
  (`proto/trogonai/session/sessions/v1alpha1/session_started.proto:16-24`).
  Strands' `Session` dataclass has no cwd/origin field of any kind
  (the dossier's [Keying and identity](./index.md#keying-and-identity) section).
- **Typed process-termination facts, kept separate from the provider-visible
  transcript.** `CommandTermination{exit_code | signal}` belongs to
  `ToolCallCompleted`, deliberately not to the result the model saw
  (`proto/trogonai/session/sessions/v1alpha1/command_termination.proto:5-22`).
  Nothing in the Strands dossier distinguishes a process exit status from any
  other tool outcome.

## Trade-offs, not gaps

- **Eager, offset-bounded resume (Strands) vs. snapshot-bounded aggregate
  replay (ours).** Strands' `initialize` calls
  `list_messages(..., offset=removed_message_count)`
  (the dossier's [Read and resume path](./index.md#read-and-resume-path) section) -- the *only* bound on resume cost is how much
  compaction has actually fired; a very long, rarely-compacted run pays a
  real, unbounded per-resume cost reading every remaining message file
  individually. Our aggregate snapshot is a genuinely separate, disposable
  artifact bounding replay cost independent of retention
  (`docs/adr/0035-session-store-decider-aggregate.md:911-914`), but the same
  open edge exists on our side for model-visible context compilation, which
  is "bounded by the latest `Compacted` marker" and not by elapsed time since
  it last fired -- an open question the Cline comparison already raised for
  our design
  (the [Cline comparison](../cline/vs-session-events.md#retention-on-an-unbounded-log)'s Retention on an unbounded log section),
  not re-derived here.
- **Whole-object atomicity-by-avoidance (Strands) vs. classified
  write-preconditions (ours).** Every Strands write is a `tempfile.mkstemp()`
  + `os.replace` (file) or a single `put_object` (S3) -- atomicity-simple,
  because a fresh whole-object write can never be observed torn
  (the dossier's [Write and append path](./index.md#write-and-append-path-ordering-durability-concurrency-delivery) section), at the cost of no server-side ordering signal
  anywhere. Ours buys real invariants on the guarded path (one active
  attempt, mutually exclusive approve/deny) at the cost of the substrate
  obligations facet 2 lists as prerequisites
  (`docs/adr/0035-session-store-decider-aggregate.md:326-341`) actually
  shipping before this store can go live. Strands' strategy is genuinely
  simpler to implement correctly on day one; ours needs more machinery but
  expresses invariants Strands' design structurally cannot, because Strands
  has no invariant-bearing writes at all.
- **Field-mutation redaction (Strands) vs. event-referencing redaction
  (ours).** Strands' `redact_message` touches exactly the one record in
  question and nothing else, which is simple to reason about for a flat
  per-message store with no forking. Ours requires the fold and every
  projection to consistently honor a masking pass over id-referenced content
  -- more moving parts, but the only shape that keeps working once
  content-addressed dedup and fork-by-reference exist, which Strands has
  neither of.

## What not to copy

- **A client-computed positional key used as the storage address.**
  `message_id` is assigned in process memory and doubles as the filename/key
  with no server check -- precisely how two independent managers resuming the
  same session collide (the dossier's [Write and append path](./index.md#write-and-append-path-ordering-durability-concurrency-delivery) section). `SessionOrdinal` is fold-derived
  and never a storage key at all, specifically so this cannot happen.
- **Optional interface methods that raise `NotImplementedError` instead of
  being excluded from the contract.** `sync_multi_agent`,
  `initialize_multi_agent`, the three bidi methods, and `delete_session`/
  `delete_message`/`delete_agent` living entirely outside the abstract
  `SessionRepository` altogether (the dossier's [The store interface](./index.md#the-store-interface) and [What this implies for our Session Store (our inference)](./index.md#what-this-implies-for-our-session-store-our-inference) sections) mean a
  conformance suite can pass against a backend that silently can't do half of
  what the interface promises. We have one storage substrate, not a
  pluggable interface, so this specific failure mode doesn't arise for us
  today; it is the right warning to keep if a pluggable
  `SessionRepository`-equivalent is ever proposed for our own store.
- **Bulk multi-key deletion with no resume marker.** Recommendation 2 above
  exists specifically because of this pattern.
- **A durable type with no version field sitting beside two adjacent
  transient mechanisms that have one.** The inconsistency itself, regardless
  of which side is "right," is what to avoid: apply one policy -- versioned
  and hard-checked, or additive-only and structurally enforced -- uniformly
  across every persistence mechanism in the platform, not just the one an
  implementer happened to design most recently.
- **Treating offset-based compaction as if it were retention.**
  `removed_message_count` only changes what a resume reads; it deletes
  nothing, ever, and there is no `delete_message` anywhere to close that gap
  (the dossier's [Compaction and history management](./index.md#compaction-and-history-management) section). Decision 7 already keeps compaction and
  retention/erasure as distinct, explicit concepts; Strands' single
  overloaded mechanism is the cautionary shape not to repeat if either is
  ever revisited.

## The two gaps the industry has not closed

### Subagent cascade

ADR#0035 decision 6 already takes a position here: a child session is its
own logical stream, linked by facts on each side
(`DelegationDispatched`/`ParentLinked`), cascade policy is explicit and
recorded (`CascadePolicy`), rewind-invalidation is distinct from terminal
cascade, and acyclicity holds by construction
(`docs/adr/0035-session-store-decider-aggregate.md:723-791`). The question
here is whether Strands' evidence validates, refines, or challenges that
position, not whether we still need one.

**What Strands does.** Both shipped multi-agent patterns actively forbid a
node's own `Agent` from carrying a session manager at all:

```python
# strands-py/src/strands/multiagent/graph.py:294-298
if isinstance(executor, Agent):
    if executor._session_manager is not None:
        raise ValueError("Session persistence is not supported for Graph agents yet.")
```

```python
# strands-py/src/strands/multiagent/swarm.py:539-541
if node._session_manager is not None:
    raise ValueError("Session persistence is not supported for Swarm agents yet.")
```

The only durable child-related artifact is the orchestrator's own
`multi_agents/multi_agent_<id>/multi_agent.json`, and it embeds only the
*last* message of each finished node via `AgentResult.to_dict()`
(the dossier's [Subagents and nested sessions](./index.md#subagents-and-nested-sessions) section) -- "lossy by construction": a node's full internal
conversation is never written to durable storage anywhere, because the
node's `Agent` is barred from having a session manager at all. Crash behavior
follows directly from the sync timing: `sync_multi_agent` fires only on
`AfterNodeCallEvent` (node completion) and `AfterMultiAgentInvocationEvent`
(run completion), so a crash while a node is *still executing* loses that
node's entire in-flight work -- nothing about it was ever synced
(the dossier's [Subagents and nested sessions](./index.md#subagents-and-nested-sessions) section). On restart, the interrupted node re-runs from scratch,
because no partial progress was ever durable.

**Does this validate, challenge, or refine decision 6?** It validates the
core premise and sharpens the actual bar decision 6 clears. Where Cline's
one-level-deep synchronous cascade showed "an incomplete cascade is dangerous
because it looks complete," Strands shows a step further: the industry's
other honest answer to "what happens to a child session's transcript" is not
an incomplete cascade at all, but *no transcript, on purpose, until this is
designed properly* -- a shipped, adopted vendor SDK voting with its feet that
decision 6's problem is hard enough to defer entirely rather than half-solve,
with the error string's "yet" as the only signal it is even on a roadmap.
This does not suggest decision 6's mechanism is wrong; it sharpens what
decision 6 has already cleared, since "does not attempt it" is itself a
competitive, currently-shipped answer. It also surfaces one narrow risk
decision 6's text does not name, though it is orthogonal to decision 6
itself: Strands' crash-mid-node loss is a durability-within-a-single-child's-
execution problem, not a linking-already-durable-children problem, and our
design structurally avoids it for a different reason -- every child gets its
own full event-sourced stream where in-flight work is durable at fact
granularity the moment each event is appended (`UserMessageRecorded`,
`ToolCallCompleted`, and so on, on the *child's own* stream), unlike Strands
where the child's durable record doesn't exist at all until the node
finishes. This is the same argument the fx comparison already made about
fact-granular commit protecting the *parent* stream from an in-flight-turn
loss on crash
(the [fx comparison](../fx/vs-session-events.md#the-one-structural-difference-everything-else-follows-from)'s structural-difference section);
Strands extends it, unintentionally, to the *child* stream specifically,
which neither fx nor Cline's dossier tested because neither product's
subagent story loses *all* in-flight child work on every crash the way
Strands' does.

### Retention on an unbounded log

ADR#0035 decision 7 already takes a position here: keep-forever,
`SessionHidden` as a visibility tombstone (no bytes deleted),
`RedactionApplied` for read-time masking, `ArtifactErased` for out-of-band
artifact-byte destruction, aggregate snapshots that "bound replay, not
storage," optional reversible cold-tiering, and erasure-grade deletion
explicitly deferred to a named follow-up ADR
(`docs/adr/0035-session-store-decider-aggregate.md:855-921`). The question
here is whether Strands' evidence validates that design or exposes a cost the
ADR does not bound.

**What Strands does.** No TTL, lifecycle policy, or scheduled cleanup exists
anywhere; a search of `session/*.py` and `types/session.py` for retention/TTL/
lifecycle/cleanup terms returns no matches (the dossier's [Retention, deletion, and multi-host](./index.md#retention-deletion-and-multi-host) section). Compaction
(`removed_message_count`) is purely a read-time offset into an untouched
message list: "No `SessionRepository` implementation has a `delete_message`
method... the underlying `message_<id>.json` files for every 'compacted-away'
turn remain on disk (or in the S3 bucket) forever." The dossier's own
example: "A session that summarizes 10,000 turns down to a 50-turn visible
window still holds 10,000 message files in storage"
(the dossier's [Compaction and history management](./index.md#compaction-and-history-management) section). Deletion exists only as `delete_session` -- whole-session,
backend-specific, not part of the abstract contract, never called by
`SessionManager` itself -- and it is the one place file and S3 genuinely
diverge in semantics: file backend `shutil.rmtree` (all-or-nothing for local
disk) vs. S3's paginated `delete_objects` loop, where "if the process crashes
or a batch call raises partway through the loop, the session is left with
some keys deleted and others present, with no recorded resume point or
partial-delete marker anywhere in the code" (the dossier's [Retention, deletion, and multi-host](./index.md#retention-deletion-and-multi-host) section).

**Does this validate, challenge, or refine decision 7?** It validates the
core keep-forever-plus-explicit-privacy-contract shape and sharpens the
deletion question specifically. Both designs accept unbounded growth --
decision 7 explicitly, Strands implicitly by never building a
`delete_message` path -- so the real divergence is entirely in what "delete"
means when someone finally asks for it. Strands attempts real erasure at the
one granularity it supports (whole session), and that attempt can silently
half-fail with no record; that is precisely the failure mode our deferred
erasure-grade-deletion follow-up ADR needs to avoid, which is why
recommendation 2 above proposes modeling it as N atomic per-item facts rather
than a bulk destroy. Where decision 7's compaction story does something
Strands' offset-only compaction cannot: our aggregate snapshot is a genuinely
separate, disposable artifact bounding replay cost independent of retention
policy, whereas Strands conflates "what the model sees" with the entire
resume mechanism, so a very long-running, rarely-compacted session pays a
real, unbounded per-resume cost reading every remaining message file
individually. This cost shape resembles Cline's field-confirmed growth
failure (`cline/cline#9011`) more than it resembles anything decision 7
produces on its own terms -- but, marking inference as inference, the Strands
dossier does not report an issue confirming this specific resume-cost failure
in the field the way Cline's was field-confirmed with open/close dates.
Strands' own `_fix_broken_tool_use` defensive-repair code, referencing GitHub
issue `strands-agents/harness-sdk#859`, is field evidence of a related but
distinct failure -- broken tool-use/tool-result pairing after truncation or a
crash -- not of a growth-driven resume slowdown specifically.

## Open questions for the ADR

1. Does cold-tier relocation to the JetStream Object Store (facet 7) need any
   write precondition of its own, or is the tiering job's single-writer
   construction sufficient justification to state explicitly that none is
   needed?
2. What is the concrete mechanism for a genuinely non-additive event-shape
   change, given facet 3 forbids per-event version branches -- a new event
   type added to the oneof, a reserved-and-replaced field number, or
   something else -- and should that mechanism be named now rather than
   invented under pressure the day it's actually needed?
3. When the deferred erasure-grade-deletion ADR is written, should it commit
   now to a per-item atomic-fact shape (extending `ArtifactErased`) rather
   than leave the door open to a bulk destroy operation later, once a batch
   API looks like the obvious performance win?
4. Should decision 6 note explicitly that a child session's own
   event-sourced stream, not just the parent-child link facts, is what
   prevents a Strands-style total loss of in-flight node work on crash? This
   is a real benefit of the current design; it is currently implicit rather
   than stated.

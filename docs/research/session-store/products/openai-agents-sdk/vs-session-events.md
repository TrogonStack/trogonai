# OpenAI Agents SDK compared to our session event catalog

Part of Session Store Research.
Produced by running [RESEARCH_PROMPT_COMPARISON](../../RESEARCH_PROMPT_COMPARISON.md).
Stage-one dossier: [OpenAI Agents SDK](./index.md).
Compared against `proto/trogonai/session/sessions/v1alpha1/` and ADR#0035 on 2026-08-04.

**Store maturity: 5/12.** Evolution scars 0/3: no schema-version field was found
"anywhere in `src/agents/memory/` or `src/agents/run_internal/session_persistence.py`"
(the dossier's [Entry/message structure and
versioning](./index.md#entrymessage-structure-and-versioning) section), no
migration file, no legacy-format sniffing, and no format-version constant appear
anywhere in the dossier. Operational age 1/3: package version `0.19.2` per
`pyproject.toml` is young; the one concrete hardening-under-load evidence found
is `SQLAlchemySession`'s bounded exponential-backoff retry loop
(`_SQLITE_LOCK_RETRY_DELAYS = (0.05, 0.1, 0.2, 0.4, 0.8)`,
`src/agents/extensions/memory/sqlalchemy_session.py:69,123-132`) built
specifically "to tolerate 'database is locked' errors," a real but narrow concurrency fix; no
issue-tracker corroboration of corruption, growth, or lock-contention failure is
cited anywhere in the dossier, unlike Cline's `cline/cline#9011`. Exposure 2/3:
vendor-shipped by OpenAI with `SQLAlchemySession`, `RedisSession`,
`MongoDBSession`, and `DaprSession` explicitly positioned as multi-process/
cloud-native-safe by the product's own docs (`docs/sessions/index.md:203-214`),
but three of those four backends were only "characterized from their headers...
and partial reads, not full line-by-line reads" (dossier, Open questions), and no
field-level adoption evidence (an issue report, a scale number) is cited anywhere.
Design independence 2/3: no evidence the store was forked from another product's
persistence code, but the dossier did not check whether the four-method `Session`
protocol shape is itself derivative of a common memory-abstraction pattern used
by comparable frameworks, so independence here is an absence-of-contrary-evidence
finding, not a directly confirmed one.

This is below the maturity threshold RESEARCH_PROMPT_COMPARISON.md sets: a
recommendation supported only by a store scoring under 6 is **thin evidence** and
must not be presented as an industry norm. Every recommendation below is weighted
down accordingly, and cross-checked against a higher-scoring product's evidence
(mostly [Cline, 10/12](../cline/vs-session-events.md)) wherever the same failure
mode recurs there, which is noted explicitly each time it happens.

## The one structural difference everything else follows from

Two products from the same vendor, in the same commit-history era, sit at
opposite ends of the append-only-log-versus-mutable-store spectrum this research
program cares about, and the divergence is not an accident of tooling age. Codex
CLI's rollout log never deletes a line; retroactive operations are appended
markers (`ThreadRolledBack`, `Compacted{replacement_history}`) interpreted at
replay (see [`../codex-cli/index.md`](../codex-cli/index.md)). This Agents SDK's
`Session` protocol has exactly four methods, `get_items`, `add_items`,
`pop_item`, `clear_session` (`src/agents/memory/session.py:13-54`, dossier
verbatim), and every one of the nine shipped backends implements `pop_item` as a
destructive row/entry delete and `run_compaction` as a destructive
`clear_session()` + `add_items()` replace. Both are OpenAI products, both are
current, both are actively maintained; the difference is a deliberate design
choice made twice by the same organization, not a legacy-versus-modern artifact.
That makes this comparison's single most useful data point not "which store is
more mature" but "what does the vendor decide differently when it optimizes for
resumable rewind-as-audit-trail (Codex CLI) versus for a narrow, backend-agnostic
storage contract many different systems can implement (this SDK)."

The structural fact everything else in this dossier follows from is where the
Agents SDK draws its store boundary: **identity, mutation-safety, and
consistency-under-retry are the caller's problem, not the store's.** The `Session`
protocol defines an operational contract (four verbs), not a data model, and it
places zero obligations on identity or idempotence. Nothing in `add_items`
promises exactly-once delivery; nothing in `pop_item` promises the deleted row
was the one the caller thinks it is once two writers race. The Runner
orchestration layer (`src/agents/run_internal/session_persistence.py`) is where
identity actually gets defended, and it defends it with a **content fingerprint**,
not a store-assigned id: `fingerprint_input_item` strips internal metadata and
optionally the `id` field, then returns `json.dumps(payload, sort_keys=True,
default=str)` (`src/agents/run_internal/items.py:334-369`, dossier verbatim); a
companion `digest_input_item` SHA-256-hashes that string "for durable occurrence
tracking" (`src/agents/run_internal/items.py:372-391`). The dossier states this
plainly: "there is no store-assigned entry id used for identity, identity for
dedup/rewind purposes is entirely content-fingerprint-based, computed by the
Runner layer, not by any backend." The Runner does not even trust its own store's
consistency under this scheme: `wait_for_session_cleanup` polls `get_items` up to
five times after a rewind "to confirm the rewound items are actually gone, rather
than assuming a strong read-after-write guarantee" (`src/agents/run_internal/session_persistence.py:624-661`, dossier). That is direct evidence, from the SDK's own code, that pushing
identity and consistency out of the store does not make either problem go away;
it moves the coping mechanism (polling, best-effort rewind that "skips the rewind
entirely with a warning if the tail doesn't match") into application code that
every caller of every backend has to trust separately.

**A semantic trap worth naming explicitly**, because it is the kind of nominal
match RESEARCH_PROMPT_COMPARISON.md's method warns is more dangerous than a gap:
our own design also keeps identity out of the domain payload, "no domain payload
gains a separate identity field of its own" (ADR#0035 decision 2). Read quickly,
that sounds like the same idea. It is not. Our runtime derives the envelope
`Event.id` deterministically, "UUIDv5 over (resolved stream subject, command
type, command idempotency key, index of the event within the decision's batch)"
(ADR#0035 decision 2), which is a function of a **caller-supplied idempotency
key**, stable across redelivery, never a function of the payload's content. The
SDK's fingerprint is a function of the **content itself**, which is exactly why
it had to grow field-stripping logic (dropping `id`, dropping "internal
metadata") to keep semantically-identical items comparable. Our scheme is immune
to that specific failure mode by construction, because identity never depends on
what the caller put in the payload; the SDK's scheme is permanently exposed to
it, because identity is nothing but a normalized view of the payload. Two
products independently discovered the same content-hash-as-identity fragility
from opposite directions worth flagging here rather than re-deriving: this SDK's
`fingerprint_input_item` strips `id`/metadata specifically so re-hashing the same
logical item after a shape change still matches, and
[Cline's `source_prefix_hash`](../cline/vs-session-events.md) "had to be
redefined mid-flight to exclude `id`/`ts`... after the team discovered hashing
transport-identity fields made projection fail for semantically identical
prefixes, so persistence was silently rejected every turn." Neither team got the
field-stripping list right on the first attempt. That is the named failure mode
of content-hash-as-identity in general: the set of fields that must be excluded
for the hash to mean "same logical thing" is itself an evolving, easy-to-get-wrong
contract, and it is invisible until a shape change breaks it in production.

## Mapping

| OpenAI Agents SDK | Ours | Verdict |
| --- | --- | --- |
| `Session.session_id: str`, caller-supplied, no minting scheme (`src/agents/memory/session.py:21`) | Opaque `SessionId`; one logical stream per session on a subject the runtime assigns (ADR#0035 decision 1) | Equivalent identity concept, opposite minting discipline: caller-chosen string vs. runtime-scoped subject |
| `Session.get_items` / `add_items` / `pop_item` / `clear_session`, four required async methods, no write precondition of any kind (`src/agents/memory/session.py:13-54`) | `decide`/`evolve`/`append_stream`, gated by a three-way `WRITE_PRECONDITION` (`NoStream`/`At(current_position)`/`Any`) per command (ADR#0035 decision 2) | Ours, decisively: every one of the SDK's four verbs is unguarded; ours classifies each command's precondition explicitly |
| `fingerprint_input_item`/`digest_input_item`: `json.dumps(payload, sort_keys=True, default=str)` plus a SHA-256 of that string, computed by the Runner, not the store (`src/agents/run_internal/items.py:334-391`) | Envelope `Event.id`, "UUIDv5 over (resolved stream subject, command type, command idempotency key, index of the event within the decision's batch)," derived by the runtime from a caller-supplied idempotency key (ADR#0035 decision 2) | Semantic mismatch, not a plain equivalence: both keep identity out of the payload, but one hashes content, the other hashes a caller-asserted key; see structural difference above |
| No listing/enumeration method anywhere in `Session`/`SessionABC`; "listing, if needed, is entirely up to the chosen backend's native tooling" (dossier, Keying and identity) | `SessionProjection` folded by `Projector::catch_up`, queried by `verb + noun` functions (`get_session`, `list_sessions`) over one rebuildable read model (ADR#0035 decision 8) | Ours, decisively: see the recommendation below |
| `pop_item()`: `DELETE FROM messages_table WHERE id = (SELECT ... ORDER BY id DESC LIMIT 1) RETURNING message_data` (`src/agents/memory/sqlite_session.py:315-326`), used for rewind | `SessionRewound{session_id, keep_through, reason}`, an appended marker; events `[1..keep_through]` remain valid, nothing is deleted (`session_rewound.proto`, ADR#0035 decision 2 and 6) | Ours, decisively: see "What not to copy" |
| `run_compaction`: `clear_session()` then `add_items()` with the compacted set, a destructive full replace (`src/agents/memory/openai_responses_compaction_session.py`) | `Compacted{session_id, summary_id, summary_content, covers_from, covers_through, trigger, ...}`, a self-sufficient in-stream marker; covered events stay on the stream (`compacted.proto`, ADR#0035 decision 4) | Ours, decisively: see "What not to copy" |
| `RunState.to_json()`/`from_json()`, an application-managed, out-of-band serialized run-state object for human-in-the-loop resume, round-tripped through the caller's own storage, never through `Session` (`src/agents/run_state.py`, dossier) | `Checkpoint{reference, checkpoint_type, digest, checkpoint_id, producing_execution_attempt_id, covers_through, session_execution_plan_digest}` inside `CheckpointProduced`, restored via `ExecutionAttemptStarted.restored_checkpoint`, digest-verified and joined by `checkpoint_id` (`checkpoint.proto`, `checkpoint_produced.proto`) | Semantic mismatch: both are called "resuming a paused run," but `RunState` is caller-owned bytes with no store relationship at all, while ours is a self-describing, store-recorded, digest-verified reference the aggregate itself validates before restore |
| `AdvancedSQLiteSession` branching: `create_branch_from_turn`, a shared-row-by-reference fork where `message_structure` indirects into shared `agent_messages` rows (`src/agents/extensions/memory/advanced_sqlite_session.py`, 808-1283 range) | `SessionForked{session_id, source_session_id, context_prefix_boundary, reason}`, an atomic `[SessionStarted, SessionForked]` batch; inheritance is by reference through the context projection, never a fold of source events into child state (`session_forked.proto`, ADR#0035 decision 5) | Ours, decisively: only one of nine backends has any fork concept at all, and it is scoped to one database, not a first-class session-store operation |
| Two subagent mechanisms, opposite durability: Handoffs share the *same* session/stream; Agents-as-tools (`Agent.as_tool`) spawns a nested `Runner.run(session: Session \| None = None)`, defaulting to no durable session at all (`src/agents/agent.py:575-597,941-953`) | `DelegationDispatched{child_session_id, operation_id, cascade_policy}` on the parent, `ParentLinked{parent_session_id, parent_dispatched_at, cascade_policy, operation_id}` on the child, always a persisted sibling stream (`delegation_dispatched.proto`, `parent_linked.proto`, ADR#0035 decision 6) | Deliberate divergence, tested against decision 6 below |
| Experimental Codex-CLI subprocess wrapper: tracks Codex's opaque `thread_id` string as an ordinary tool-output item, "the only thing that crosses from Codex's world into the Agents SDK's session," no shared format (`src/agents/extensions/experimental/codex/`, dossier) | `ExternalDelegationDispatched{operation_id, delegate_reference, authenticated_remote_subject, authorization_reference, request_digest, correlation_id}` on the dispatching session's own stream (`external_delegation_dispatched.proto`, ADR#0035 decision 6) | Ours, decisively: the SDK's own cautionary example is an unaudited opaque string; ours records exactly the evidence ADR#0031 requires for the same kind of cross-store delegation |
| `TResponseInputItem = ResponseInputItemParam`, a type alias onto the OpenAI Python SDK's own wire type, no Agents-SDK-level envelope, no schema-version field anywhere (`src/agents/items.py:76`, dossier) | `CanonicalMessage{message_id, role, content, model, usage, created_at}` with a typed `ContentBlock` oneof (`text`, `artifact_ref`, `ThinkingBlock`, `ToolUseBlock`, `ToolResultBlock`, `bytes redacted_thinking`, `ProviderBlock`) (`message.proto`) | Ours, decisively: a normalized, model-agnostic shape with an explicit unmodelled-provider escape hatch (`ProviderBlock`), versus a bare alias onto a third party's wire type with no version field of our own |
| `SQLiteSession` corrupt-row handling: silently `continue`s past undecodable rows on read, then doubles the read window and retries until `limit` valid items are returned (`src/agents/memory/sqlite_session.py:218-263`) | A decode-failure metric is a stated substrate obligation (ADR#0035 decision 2, Substrate obligations) | Ours, decisively: a malformed event is observable, not silently absorbed by a widened read window |
| `MongoDBSession` message `seq`, "an atomic sequence counter" attached per document, needed because Mongo has no auto-increment primary key (`docs/sessions/index.md:209,453`) | `SessionOrdinal`, the 1-indexed fold-derived position of an already-appended event, "never read from JetStream message metadata... stable across restore, backfill, migration, and cold-tier relocation" (`session_ordinal.proto`, ADR#0035 decision 2) | Ours, decisively: a fold-derived logical position survives storage relocation; a stored counter value is only as consistent as the increment operation that assigned it |
| `SessionSettings.limit` / `RunConfig.session_input_callback`, explicitly decoupling what the model sees this turn from what the store holds (`src/agents/memory/session_settings.py`, `src/agents/memory/util.py:8-11`) | Model-visible context "compiled deterministically from the event log bounded by the latest `Compacted` marker," a read-side projection over the full log (ADR#0035 decision 8) | Equivalent in spirit: both treat "what the model reads" as a bounded view derived from an unbounded durable record, computed independently of each other |
| `DaprSession(ttl=...)`, the one backend with native TTL; `EncryptedSession`'s own `ttl` layered on any backend, expired entries silently skipped on decrypt, not purged (`docs/sessions/index.md:418`, `src/agents/extensions/memory/encrypt_session.py`) | `SessionHidden{reason}` (visibility tombstone, no bytes deleted), `RedactionApplied{redacted_event_ids, reason}` (read-time masking), `ArtifactErased{artifact_id, reason}` (out-of-band byte destruction) (ADR#0035 decision 7) | Trade-off, not a plain win: see the retention gap below, the SDK's `SQLiteSession.clear_session()` also does something ours deliberately does not, real physical deletion |

## What we should consider changing

### 1. Name, in the ADR or in `checkpoint_produced.proto`'s comment, exactly which bytes the checkpoint evidence digest covers

**The change.** ADR#0035 decision 2's fold rule for `CheckpointProduced` states
"the command idempotency key includes the artifact digest, so conflicting
evidence remains visible while byte-identical redelivery collapses through the
event identity contract." Neither `checkpoint.proto` nor the decision text pins
down, in writing, which fields of the checkpoint artifact are included in that
digest and which (if any) are excluded as volatile.

**Evidence anchor.** This SDK, store maturity 5/12 (thin evidence on its own):
`fingerprint_input_item` "strips internal metadata and (optionally) the `id`
field" before hashing (`src/agents/run_internal/items.py:334-369`), a fix that
exists only because an earlier version of the same idea did not strip those
fields and broke identity across semantically-identical items. Corroborated by
a materially stronger source, [Cline (10/12)](../cline/vs-session-events.md):
its `source_prefix_hash` "had to be redefined mid-flight to exclude `id`/`ts`...
after the team discovered hashing transport-identity fields made projection
fail for semantically identical prefixes, so persistence was silently rejected
every turn." Two unrelated teams hit the identical bug class from opposite
starting points; this is not thin evidence once the two are read together, even
though neither alone would justify the recommendation.

**Blast radius.** Additive if the digest's covered-bytes contract is already
correct and this only writes the contract down; breaking, cheap if today's
digest computation turns out to include a volatile field (for example a
wall-clock timestamp on the checkpoint artifact), since fixing that only
changes what future digests are computed over, with no persisted-event rewrite
needed (old `CheckpointProduced` events keep whatever digest they were written
with; only the *comparison* rule for new evidence changes).

**Why.** `Checkpoint.digest` (`checkpoint.proto`) is a `Digest` over "the
checkpoint bytes," and the first-evidence-per-checkpoint-id fold rule depends on
byte-identical redelivery actually producing the same digest. If the artifact
serialization the digest covers ever grows a field that varies harmlessly
between two logically-identical checkpoints (a producing timestamp, a
serializer's map key ordering before it was pinned), the fold would treat
identical restarts as *conflicting* evidence rather than a duplicate, silently
defeating the "first evidence wins, retained for audit" guarantee the decision
promises. This has not happened to us yet, by design it cannot be observed until
it does, which is exactly the shape of both failure reports cited above.

**Cost.** Writing the contract down costs a paragraph. If it surfaces an actual
bug in what the digest covers today, the cost is redefining the digest input for
new checkpoints going forward, a serializer-level change, not a proto change.

### 2. State explicitly whether a delegated child session may skip persistence entirely for short-lived, tool-like nested invocations

**The change.** ADR#0035 decision 6 models every delegated child as a persisted
sibling stream: `DispatchDelegation` always mints a fresh `child_session_id` and
always creates a real `[SessionStarted, ParentLinked]` batch. Nothing in the
decision says whether a lightweight, tool-like nested agent invocation is
expected to go through this path at all, versus staying inside the parent's own
`ToolCallRequested`/`ToolCallCompleted` pair with no child session minted.

**Evidence anchor.** This SDK, store maturity 5/12 (thin evidence): its
agents-as-tools mechanism (`Agent.as_tool`, `src/agents/agent.py:575-597`) spawns
a fully separate nested `Runner.run(session: Session | None = None)`, defaulting
to `None`; "a nested agent-as-tool run has no durable session at all unless the
caller explicitly constructs and passes one... only the nested run's final
output string round-trips back into the parent's session, as an ordinary
tool-call-output item" (dossier, Subagents and nested sessions). Handoffs, the
SDK's other subagent mechanism, take the opposite position: they never leave the
parent's stream at all ("the mapped history is the exact model input, new items
stay unchanged for session history," `src/agents/handoffs/history.py:151-152`).

**Blast radius.** Additive. Nothing in `DispatchDelegation`'s definition forces
a caller to invoke it for every nested-agent call; a harness that wants
ephemeral, tool-like subagent semantics can already keep the whole interaction
inside ordinary `ToolCallRequested`/`ToolCallStarted`/`ToolCallCompleted` events
on the parent's own stream today, with no new event type and no schema change.
What is missing is not a mechanism, it is the ADR stating this is the *intended*
default for that case, so a future implementer does not assume
`DispatchDelegation` is mandatory for every nested-agent invocation regardless
of how short-lived or tool-like it is.

**Why.** Decision 6's transitive cascade and two-fact detach saga are real
machinery, worth their cost for a genuinely independent, long-lived,
resumable child session. They are needless overhead for a nested call whose
entire lifecycle is "ask a sub-agent a question, get a string back, done,"
which is exactly the shape a tool call already models. The SDK's `session=None`
default is a real, shipped answer to "should every nested invocation be a
first-class session," and the answer it gives is no. Handoffs' opposite answer
(no separation at all, same stream) suggests the more general point: not every
subagent relationship is well-modeled as a sibling stream, and decision 6
should say which shape of nesting it is scoped to rather than reading as
universal.

**Cost.** None beyond the decision text itself. If the ADR wants to go further
and formalize "no session" as a first-class option (rather than "just don't call
`DispatchDelegation`"), that would need a documented convention for how such a
nested call's context is recorded on the parent (likely already covered by
existing `ToolCallRequested`/`Completed` fields), which is a design discussion,
not a schema change on its own.

### 3. Do not let a future listing/search projection fragment into per-backend query surfaces

**The change under consideration, and why to reject it.** Nothing in ADR#0035
proposes this today; this recommendation exists to record why it should stay
rejected. A tempting shortcut for a future feature (project-scoped listing, a
picker UI) is to let each storage backend or deployment expose its own native
query surface directly, the way this SDK does.

**Evidence anchor.** This SDK, store maturity 5/12: "there is no
`list_sessions`/`list_session_ids` method... a repo-wide search for such names
in `src/agents/` returns no hits... listing, if needed, is entirely up to the
chosen backend's native tooling (e.g. querying the SQLite `agent_sessions` table
directly, or the Mongo `agent_sessions` collection)" (dossier, Keying and
identity, and Listing/summaries/search). The result is nine backends with no
shared listing contract at all; a caller who switches backends loses whatever
listing code they had built.

**Blast radius.** N/A, this recommends holding the line on an existing decision,
not changing anything.

**Why not to do this.** ADR#0035 decision 8 already answers this correctly:
listing is a rebuildable `SessionProjection`, queried by `get_session`/
`list_sessions`, never the backend's native storage medium. The SDK is the
clearest cautionary counterexample available in this corpus for why: a store
with no listing contract does not have "flexible, pluggable listing," it has
nine independently-reinvented, backend-coupled listing implementations, none of
which is portable if the backend ever changes. Recording this here is meant to
stop a future "just query NATS/KV directly for this one dashboard" shortcut from
being proposed as a small convenience; it is the exact shape of the gap this
product actually shipped.

**Cost.** None; this is a reaffirmation, not a new obligation.

## What our design already does better

- **Server-side write preconditions vs. no write contract at all.** Every one
  of the `Session` protocol's four methods (`get_items`, `add_items`,
  `pop_item`, `clear_session`) is unguarded; concurrency is left entirely to
  the backend, and the base `SQLiteSession` backend has "no `PRAGMA
  busy_timeout` set anywhere" and no cross-process defense against
  `SQLITE_BUSY` at all (dossier, Write and append path). Our `WRITE_PRECONDITION`
  classification (`NoStream`/`At`/`Any`, ADR#0035 decision 2) is enforced by the
  broker for every invariant-bearing transition, not assumed away by whichever
  backend a caller happened to configure.
- **Identity that survives a payload's content changing.** As detailed above,
  our envelope `Event.id` is derived from a caller-supplied idempotency key,
  never from the payload's bytes, so it cannot be broken by the exact class of
  bug that hit both this SDK's `fingerprint_input_item` and Cline's
  `source_prefix_hash` independently.
- **A decode-failure metric, not a silently widened read window.**
  `SQLiteSession.get_items(limit=N)` skips corrupt rows during decode and
  doubles its read window until it finds `N` valid items
  (`src/agents/memory/sqlite_session.py:218-263`), with no signal to the caller
  that anything was skipped. Our decode-failure metric (ADR#0035 decision 2,
  Substrate obligations) makes a malformed event observable instead of quietly
  compensated for.
- **Rewind and compaction as appended facts, not destructive operations.**
  `pop_item`'s `DELETE ... RETURNING` and `run_compaction`'s
  `clear_session()` + `add_items()` both destroy the prior state; our
  `SessionRewound` and `Compacted` are both appended markers interpreted at
  replay, and the covered events "stay on the stream (keep-forever)"
  (`compacted.proto`).
- **A single, reused `Digest` type versus a one-off hashing utility.** Our
  `Digest{algorithm, value}` backs `ArtifactRef`, `Checkpoint`,
  `ResourceObservation`, and `OperationOutcomeRecorded` uniformly. This SDK's
  content-hashing (`digest_input_item`) exists in exactly one place, purpose-built
  for Runner-internal dedup, with no shared type or reuse across the rest of the
  codebase.
- **A named escape hatch for content we do not understand yet.** `ProviderBlock`
  (`message.proto`) lets a new provider-specific content shape be recorded
  without a schema change; `ResponseInputItemParam`'s shape is entirely owned by
  the `openai` package's own versioning, with "no schema-version field... found
  anywhere" on the Agents SDK's own side to track drift in that upstream type.
- **Cross-store delegation with an evidentiary contract.** `ExternalDelegationDispatched`
  records the authenticated remote subject, authorization reference, and a
  request digest for any delegate outside our own store. This SDK's only
  cross-store bridge (the Codex-CLI subprocess wrapper) crosses as "an opaque
  string value inside an ordinary tool-output item," with none of that evidence
  captured anywhere.

## Trade-offs, not gaps

- **Nine pluggable backends vs. one substrate-guaranteed store.** The `Session`
  protocol's four-method minimalism is what lets it be implemented over SQLite,
  Postgres, MySQL, Redis, MongoDB, Dapr, and OpenAI's own Conversations API with
  no changes to the Runner. That same minimalism is why none of those backends
  gets a shared identity, ordering, or concurrency guarantee for free; each one
  re-derives its own answer (an autoincrement column, a Mongo `seq` counter, a
  Dapr ETag), several imperfectly (`SQLiteSession` has no cross-process busy
  handling at all). Our design buys uniform, substrate-enforced guarantees at
  the cost of being one store, on one substrate, not a portable interface many
  storage technologies can each independently satisfy.
- **Opaque JSON blobs vs. schema-validated typed protobuf.** `SQLiteSession`
  treats every item as an opaque blob, `json.dumps`/`json.loads` with no
  validation (`src/agents/memory/sqlite_session.py:189,222`); this is precisely
  what lets the same store code serve any `TResponseInputItem` shape the
  installed `openai` package happens to define, with zero coupling to a schema
  we would have to keep in sync. Our decision 3 schema-validates every event at
  the storage boundary, which catches malformed events early at the cost of
  every event type needing an explicit, versioned proto definition before it can
  be recorded.
- **Physical deletion vs. keep-forever masking.** `SQLiteSession.clear_session()`
  issues real `DELETE` statements against both tables, no soft-delete
  (`src/agents/memory/sqlite_session.py:359-374`); `DaprSession`'s TTL actually
  expires data at the storage layer. Our `SessionHidden`/`RedactionApplied`
  keep-forever-and-mask design (decision 7) buys full audit history and
  fork-safe redaction (masking a source stream automatically masks every fork's
  inherited context, since a fork reads by reference) at the cost of not
  offering real physical erasure today; that gap is explicitly named, not
  accidental, and is tested against this SDK's evidence below.

## What not to copy

- **`pop_item`'s destructive `DELETE ... RETURNING` as a rewind primitive.**
  This directly contradicts ADR#0035 decision 2's forced position that "every
  retroactive operation, rewind, revert, compaction, hide, is a new appended
  event interpreted at replay, never an edit or a delete of stored messages."
  It also has a demonstrated correctness cost inside the SDK itself: rewind is
  "explicitly best-effort," matching a content-fingerprinted tail and *skipping*
  the rewind with only a warning if the match fails, and the orchestration layer
  has to poll `get_items` up to five times afterward "rather than assuming a
  strong read-after-write guarantee."
- **`run_compaction`'s destructive `clear_session()` + `add_items()` replace.**
  This is the exact opposite of decision 4's in-stream marker; it leaves "no
  trace of the pre-compaction items in the store once it completes
  successfully," which is unrecoverable if the compaction summary itself turns
  out to be wrong, and it blocks completion of the run while it happens ("the
  SDK waits for compaction to finish before considering the run complete,"
  `docs/sessions/index.md:284-285`).
- **Content-fingerprint-as-identity for anything durable.** As detailed above,
  hashing normalized payload content to stand in for identity requires an
  evolving, easy-to-get-wrong field-exclusion list, and has already broken in
  production for two unrelated teams (this SDK's `fingerprint_input_item`,
  Cline's `source_prefix_hash`). Wherever our own design computes a content
  digest for comparison purposes (`ResourceObservation.content_digest`, the
  artifact digest feeding a `CheckpointProduced` idempotency key), the covered
  bytes must be pinned down explicitly, per recommendation 1, rather than left
  to whatever a serializer happens to emit.
- **Letting listing be "whatever the backend's native tooling supports."** Nine
  backends, nine incompatible listing stories, none portable across a backend
  change. Decision 8's single rebuildable `SessionProjection` is the fix; do not
  let a future feature carve a shortcut back to per-backend native queries.
- **A four-method contract with zero version discipline on the item shape it
  carries.** `TResponseInputItem` is a bare alias onto a third-party package's
  wire type, with "no schema-version field... found anywhere"; format evolution
  is implicitly whatever the `openai` package's own versioning happens to do.
  We already made the opposite call (decision 3, typed protobuf, schema-validated
  at the boundary); nothing here argues for revisiting that.

## The two gaps the industry has not closed

### Subagent cascade

ADR#0035 decision 6 already takes a position: a delegated child is always its
own logical stream, linked by `DelegationDispatched`/`ParentLinked` on each
side, with a `CascadePolicy` (`CASCADE_ON_PARENT_TERMINAL` or `INDEPENDENT`)
governing what happens on parent termination, and `ParentHistoryInvalidated`
handling the separate case of a parent rewind that invalidates a child's
dispatch point without the parent being terminal. The question here is whether
this SDK's evidence validates, refines, or challenges that position, not
whether we still need one.

**What this SDK does.** It has two subagent mechanisms with opposite answers,
and neither one matches the sibling-stream-plus-pointer shape convergence #6
in the [cross-product synthesis](../../synthesis.md) found almost everywhere
else in the corpus. Handoffs never leave the parent's own stream at all, "the
mapped history is the exact model input, new items stay unchanged for session
history" (`src/agents/handoffs/history.py:151-152`), so there is no separate
child to cascade anything to; the parent-child question simply does not arise.
Agents-as-tools spawns a genuinely separate nested `Runner.run`, but its
`session` parameter "defaults to `None`," and when it does, "a nested
agent-as-tool run has no durable session at all... only the nested run's final
output string round-trips back into the parent's session." On parent crash,
directly quoting the dossier's code-path finding rather than its docs: "no code
path was found that deletes, orphans, or reconciles a nested session on parent
crash, the SDK does not model a parent-child session relationship as a
first-class concept at all." Even in the one case a caller *does* pass an
explicit `session=` for a nested agents-as-tool call, "the SDK does not
establish or track any parent-child link between the parent's session and that
child session" (dossier, marked **[inference]** by the dossier itself, since no
negative-existence proof is absolute); the durable session, if one exists at
all, is simply an unrelated `Session` instance the caller happens to own.

**Does this validate, refine, or challenge decision 6?** It refines it by
surfacing a third position the rest of the corpus had not shown. Every other
product's subagent mechanism at least records a parent pointer even where
cascade-on-delete is unhandled (synthesis convergence #6 and #7: "every product
that has subagents has the same unresolved gap," meaning an orphaned pointer,
not a missing one). This SDK's agents-as-tools default is a level further back:
by default, there is no persisted child at all to orphan, because nothing is
dispatched into a session-store concept in the first place; nesting is treated
as pure runtime state unless a caller opts in. That is not evidence against
decision 6's design for the case it targets (an independently resumable,
audited child session), it is evidence that decision 6 currently reads as if
*every* nested agent invocation should go through that machinery, when a real,
shipped product from a major vendor treats "no durable session at all" as the
correct default for a large, common class of nested calls (short-lived,
tool-like, single-question-single-answer). Recommendation 2 above is the
concrete response: state explicitly that skipping `DispatchDelegation` for that
class of call is the intended, supported behavior, not an oversight to close.
Handoffs' opposite answer (share the exact same stream, no separation
whatsoever) is a genuinely different case our catalog does not model at all;
whether "a different agent config takes over the same conversation" belongs in
the Session Store's scope or is purely an agent-loop concern is left as an open
question below rather than resolved here, since nothing in the dossier or our
own ADR speaks to it directly.

### Retention on an unbounded log

ADR#0035 decision 7 already takes a position: keep-forever, with `SessionHidden`
as a visibility tombstone that "does not promise erasure the log does not
perform," `RedactionApplied` for read-time masking that "the fold and every
projection" apply while "original bytes remain on the keep-forever log," and
`ArtifactErased` for out-of-band artifact-byte destruction with the artifact's
digest and metadata staying on the log as provenance. Erasure-grade deletion
(cryptographic shredding) is explicitly deferred to a named follow-up ADR. The
question is whether this SDK's evidence validates that interim posture or
exposes a cost it does not bound.

**What growth looks like in this SDK, quoting the code path.** "There is no
compaction in the store/protocol layer, `Session`, `SessionABC`, and every
plain backend... hold every item indefinitely; nothing trims them" (dossier,
Compaction and history management). `SessionSettings.limit` bounds what is read
into the model context per turn, "the underlying row/document count is
unaffected." The only thing that actually shrinks storage is
`OpenAIResponsesCompactionSession`, an opt-in decorator whose mechanism is the
destructive `clear_session()` + `add_items()` already flagged above, which
means the *only* built-in bound on this SDK's storage growth is destructive by
construction; there is no non-destructive compaction option at all. No
GitHub-issue-level corroboration of growth becoming user-visible (comparable to
Cline's `cline/cline#9011`) is cited anywhere in this dossier; that absence is
itself an open item, not a confirmed "no failures happened," since the dossier
did not investigate the `openai-agents-python` issue tracker for this pass.

**What deletion looks like, the sharper angle this product actually adds.**
Unlike Cline (which never gave this comparison a real physical-delete
primitive to weigh against decision 7), this SDK does: `SQLiteSession.clear_session()`
issues real `DELETE FROM messages`/`DELETE FROM sessions` statements, "no
soft-delete" (`src/agents/memory/sqlite_session.py:359-374`), and `DaprSession`'s
TTL actually expires state-store entries. That is a real, shipped, working
full-erasure primitive, crude (it is all-or-nothing per session, no selective
redaction, no provenance kept afterward) but genuinely present today, not
deferred to a future ADR the way ours is.

**Does this validate, refine, or challenge decision 7?** It is a genuine but
thin-evidence challenge, not a validation, and the ADR should read it as such
given the 5/12 score. On the growth axis, this SDK's evidence is weak: it shows
the same "the two purest event-sourced products have no retention story"
pattern the synthesis already names for T3 Code and OpenCode, generalized to
"the two products with no meaningful append-only discipline at all also have no
non-destructive retention story," which is consistent with our decision 7's
premise that keep-forever needs a deliberate redaction/erasure contract rather
than something the pattern gives you for free, so nothing here argues for
changing decision 7's shape. On the erasure axis, though, the challenge is real
even if the evidence backing it is thin: decision 7's interim posture (masking
now, cryptographic shredding deferred) is a genuine capability gap against a
product that already ships full physical deletion today, however crude. This
does not mean copy `clear_session()` (see "What not to copy": all-or-nothing,
no selective redaction, no fork-safety, no provenance); it means the named
follow-up ADR for erasure-grade deletion is not a nice-to-have relative to the
rest of the industry, at least one shipped, vendor-backed alternative already
offers a cruder version of exactly the capability decision 7 defers.

## Open questions for the ADR

1. Should the ADR state explicitly that a caller may choose not to call
   `DispatchDelegation` for a short-lived, tool-like nested agent invocation,
   keeping it inside the parent's own `ToolCallRequested`/`ToolCallCompleted`
   pair with no child session minted at all, the way this SDK's
   agents-as-tools default (`session=None`) effectively does? See
   recommendation 2 and the subagent-cascade section above.
2. Is a "different agent configuration continues the same conversation, no
   session boundary at all" pattern (this SDK's Handoffs) in scope for the
   Session Store, or is it purely an agent-loop concern the store never needs
   to represent? Nothing in ADR#0035 or this dossier answers this directly.
3. What bytes does the artifact digest feeding a `CheckpointProduced` command's
   idempotency key actually cover, and are any of them volatile (a
   producing-side timestamp, a non-canonical serialization) in a way that could
   make byte-identical checkpoints hash differently? See recommendation 1.
4. Given decision 7 defers erasure-grade deletion to a named follow-up ADR, and
   at least one shipped vendor product already offers a cruder but real
   physical-delete primitive today, should that follow-up ADR be prioritized
   ahead of other open work, or is the masking-plus-tombstone interim story
   considered sufficient for the deployments this store targets in the near
   term?

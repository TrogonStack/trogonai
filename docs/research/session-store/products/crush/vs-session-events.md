# Crush compared to our session event catalog

Part of Session Store Research.
Produced by running [RESEARCH_PROMPT_COMPARISON](../../RESEARCH_PROMPT_COMPARISON.md).
Stage-one dossier: [Crush](./index.md).
Compared against `proto/trogonai/session/sessions/v1alpha1/` and [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) on 2026-08-04.

**Store maturity: 9/12**: evolution scars 2/3 (7 goose migrations add columns
across time and carry existing rows forward via plain `ALTER TABLE ... ADD
COLUMN` statements, for example `internal/db/migrations/20250810000000_add_is_summary_message.sql`
and `internal/db/migrations/20250627000000_add_provider_to_messages.sql`;
capped below 3/3 because the `messages.parts` JSON envelope itself carries no
schema-version field and no sniffing/back-compat read path, a gap the dossier
flags against itself), operational age 2/3 (a documented production incident
shaped the concurrency design: `internal/db/connect.go:137-141` cites
"WAL/header desync resulting in SQLITE_NOTADB (26) on the next open" as the
reason `SetMaxOpenConns(1)` exists; no first-commit date or issue-tracker
citation was available in the dossier to establish total field time), exposure
2/3 (a vendor-shipped Charmbracelet CLI/TUI product with a local REST/IPC
surface for multi-client-same-host use, but the dossier is explicit that this
is "not a first-class remote path," with no multi-host or network-filesystem
handling found anywhere), design independence 3/3 (an original Charmbracelet
schema and service layer, not inherited from an upstream fork).

## The one structural difference everything else follows from

Crush persists **current state**, not **history**. A stored session is a
mutable `sessions` row that *is* the state (title, token/cost counters,
`summary_message_id`, `todos`, all updated in place via full-row `UPDATE`,
`internal/db/sql/sessions.sql:43-53, 55-63`), plus a collection of `messages`
rows each individually mutable in place (`parts` is wholesale-overwritten by
`UpdateMessage`, `internal/message/message.go:403-411`), plus exactly one
genuinely append-only child collection, the per-path `files` version log. The
dossier's own conclusion is direct on this point: "there is no single
authoritative append-only log underneath; instead there are three
independently-authoritative, differently-mutable stores... unified only by
foreign keys and a shared connection." Crush sits in the cross-product
[synthesis](../../synthesis.md)'s "session-as-row" category alongside Goose and
Hermes, the two products that synthesis names as "the least event-sourced of
the products studied."

We persist facts and derive state by folding them. `UserMessageRecorded`,
`AssistantMessageCompleted`, `ToolCallCompleted`, and every other arm of the
`SessionEvent` oneof are immutable once appended; the session's current title,
cost, summary boundary, and cascade state are never stored anywhere as a
mutable field, they are the output of folding the stream at read time
([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) facet 2, facet 8).

This is a different, more fundamental axis than the one the
[fx comparison](../fx/vs-session-events.md) leads with. fx's structural
difference is a question of commit granularity (turn versus fact) inside an
append-only design fx and we both already share. Crush's is a question of
whether an append-only design exists at all, and it does not, below the
`files` table.

Nearly everything else in this comparison is a consequence of that one
choice, not an independent difference:

- **No expected-version precondition anywhere** (confirmed by the dossier's
  own explicit grep for CAS/compare-and-swap patterns across
  `internal/session`, `internal/message`, `internal/history`) follows from
  there being no single log whose position a caller could stake a claim
  against. The closest analog, `history.service.createWithVersion`'s
  retry-on-`UNIQUE`-constraint loop (`internal/history/file.go:84-135`),
  auto-increments a version on conflict rather than checking one the caller
  supplied, which is retry-on-collision, not optimistic concurrency.
- **The client-buffered, debounce-then-flush write path** for messages
  (`internal/message/message.go:21`, `:31-45`) exists because a message row
  is a thing you overwrite, not a thing you append to. A durable fact never
  needs a "flush before reading" contract, because it was already durable the
  instant it was appended.
- **The central cascade finding**, `parent_session_id`'s missing foreign key,
  is a symptom of the same model: a relationship between two mutable rows is
  exactly as durable as someone remembering to maintain it, whereas a
  relationship recorded as an event (`DelegationDispatched` / `ParentLinked`)
  is a fact a reconciler can rediscover and repair after a crash.
- **Retention has no in-between state.** There is no masked-but-retained
  status because there is no log to redact, only rows to delete or keep;
  "delete" in Crush can only mean a physical SQL `DELETE`, never a visibility
  tombstone over an otherwise-preserved fact.

## Mapping

| Crush | Ours | Verdict |
| --- | --- | --- |
| `sessions` row (title, `message_count`, `prompt_tokens`, `completion_tokens`, `cost`, `summary_message_id`, `todos`), mutated via full-row `UPDATE` | Fold of `SessionStarted` + `TokenUsage`-bearing events + `SessionRenamed` + `TodoUpdated` + `Compacted` over the session stream | Ours |
| `uuid.New().String()` session id (`internal/session/session.go:98`) | Opaque `session_id` ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) does not mandate a UUID version) | Equivalent |
| `parent_session_id TEXT`, no FK, no application-level cascade (`internal/db/migrations/20250424200609_initial.sql:6`) | `DelegationDispatched` + `ParentLinked` + `CascadePolicy`, reconciler-enforced | Ours, decisively (see gap section) |
| Tool-call-ID reused as subagent session id (`CreateTaskSession`, `internal/session/session.go:110-122`) | `child_session_id` is always a freshly minted id in a distinct namespace from `tool_call_id` (`DelegationDispatched.child_session_id`) | Ours (see What not to copy) |
| `"title-" + parentSessionID` deterministic composite session id (`internal/session/session.go:124-136`) | No equivalent; nothing in our catalog derives one entity's id from another's | Ours (see What not to copy) |
| `CreateAgentToolSessionID(messageID, toolCallID)` → `"%s$$%s"` composite string (`internal/session/session.go:351-362`) | No equivalent; `turn_id` and `tool_call_id` are already distinct typed correlators | Ours |
| `messages` row, `parts` JSON array wholesale-overwritten via `UpdateMessage` (`internal/message/message.go:403-411`) | `UserMessageRecorded` / `AssistantMessageStarted` / `AssistantMessageCompleted` / `AssistantMessageFailed`, each an immutable append | Ours, decisively |
| `ContentPart` union: `ReasoningContent`, `TextContent`, `ImageURLContent`, `BinaryContent`, `ToolCall`, `ToolResult`, `Finish`, `ShellCommand` (`internal/message/content.go:55-146`) | `ContentBlock` oneof (`ThinkingBlock`, `ToolUseBlock`, `ToolResultBlock`, `ProviderBlock`) on `CanonicalMessage` | Mostly equivalent; `ShellCommand` as a distinct message-content type has no analog (see Open questions) |
| `Finish{Reason, Time, Message, Details string}` | `AssistantMessageFailed` / `SessionCancelled.reason` / `SessionFailed.reason`, each a typed enum plus a free-text detail | Ours (typed reason vs. an untyped `Details` string) |
| `files` row: `session_id, path, content TEXT, version`, `UNIQUE(path, session_id, version)`, `ON DELETE CASCADE` (`internal/db/migrations/20250424200609_initial.sql:24-34`) | `FileChanged{path, change_kind, before_ref, after_ref, tool_call_id, turn_id, diff}`, content-addressed via `ArtifactRef`/`Digest` | Ours, decisively |
| `read_files` upsert of `read_at` per `(path, session_id)` (`internal/db/sql/read_files.sql:1-11`) | `ResourceObservation{uri, content_digest \| absent, range, complete}` on `ToolCallCompleted.observed` | Ours (digest and coverage, not just a timestamp; see already-does-better) |
| `sessions.summary_message_id` + `messages.is_summary_message`, truncation applied only inside `getSessionMessages` (`internal/agent/agent.go:1692-1711`) | `Compacted{summary_id, summary_content, covers_from, covers_through, trigger, guidance, tokens_before, tokens_after, model, usage}` | Ours, decisively stronger (see already-does-better) |
| No session-level rewind/checkpoint/fork of any kind (confirmed by a targeted grep for `rewind`, `checkpoint`, `fork`/`Fork`) | `SessionRewound{keep_through}`, `Checkpoint`/`CheckpointProduced`, `SessionForked` | Gap in Crush, not in us |
| Per-file version rows tied to tool calls, write-only in this codebase (no "restore to version N" caller found) | `Checkpoint.covers_through` restored via `ExecutionAttemptStarted.restored_checkpoint`, digest-verified | Ours |
| `crush stats`: read-only, filesystem-crawled, cross-project aggregation (`internal/cmd/stats.go:243-388`) | Not modeled; listing/search/analytics are rebuildable projections outside the event catalog ([ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) facet 8) | Neutral, out of scope on both sides |
| No FTS/search subsystem found | Not modeled in the catalog either (facet 8) | Neutral |
| `sessions.updated_at`/`created_at`, Unix-epoch wall-clock integers, no monotonic sequence | `SessionOrdinal`, fold-derived logical position | Ours, decisively |
| No optimistic-concurrency/expected-version precondition anywhere (confirmed by explicit grep in the dossier) | `WRITE_PRECONDITION` (`NoStream` / `At` / `Any` classification, [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) facet 2) | Ours, decisively |
| `SetMaxOpenConns(1)` plus an opt-in, same-host advisory `flock` (`internal/db/datadirlock.go:51-80`, enabled only on the server-bootstrap path) | Per-subject expected-sequence enforcement, server-side, unconditional | Ours |
| No client-supplied idempotency key anywhere; every write mints a fresh server-side UUID | `OperationReserved.request_digest`, deterministic event ids (facet 2, facet 3) | Ours, decisively |
| `session.service.Delete`: real transactional SQL `DELETE` of the session's own `messages`/`files`, no walk of children (`internal/session/session.go:138-169`) | `SessionHidden` (visibility tombstone only) + `RedactionApplied` + `ArtifactErased`; the log itself is never truncated | Semantic mismatch, not a strict ranking either way (see Retention gap section) |
| Provider-specific fields folded directly into the canonical struct: `ReasoningContent.ThoughtSignature // Used for google`, `ReasoningContent.ToolID // Used for openrouter google models` (`internal/message/content.go:55-146`) | `ProviderBlock` (write-verbatim, read-never) and `ThinkingBlock.signature` as a contained escape hatch | Ours (see What not to copy) |
| `messages.model`, `messages.provider` only; no temperature/effort/thinking-budget recorded anywhere | `ModelSettings{max_output_tokens, temperature, top_p, thinking_budget_tokens, stop_sequences, raw_settings}` on `AssistantMessageStarted` | Ours, decisively |
| Project scope = one `crush.db` per project directory, resolved via `fsext.LookupClosestBounded` (`internal/config/load.go:556-559`); no `workspace_id` column exists | `WorkspaceRef` inline on `SessionStarted` inside one shared store | Trade-off (see below) |
| `crush.lock` OS advisory flock plus a `dataDirOwnerInfo` JSON crash-liveness payload (informational only; `internal/db/datadirlock.go:26-33, 73-78, 88-98`) | Server-enforced `WRITE_PRECONDITION`, no client-side lock file at all | Ours |

## What we should consider changing

Ordered most-consequential first.

### 1. Add an explicit invariant: no session id may ever be derived from another entity's id

**The change.** State, as a testable rule in [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) (a natural home is
alongside facet 2's identity/dedup contract, or facet 6's "always mints a
fresh `child_session_id`"), that a `session_id` must never be generated
deterministically from, or reused directly as, any other entity's id
(a `tool_call_id`, a `message_id`), and validate it at the command boundary
(facet 3 already validates commands generally).

**Evidence anchor.** Crush (9/12), `internal/session/session.go:110-122`:
`CreateTaskSession` sets `ID: toolCallID` directly, so a running subagent's
session id *is* a tool-call id from a different lifecycle entirely; and
`internal/session/session.go:124-136`: `CreateTitleSession` mints the
deterministic, collidable key `"title-" + parentSessionID`, with the
dossier's own Open Questions noting that a second call for the same parent
was "not traced into `CreateSession`'s SQL," leaving the collision behavior
unverified even by the people who wrote it.

**Blast radius.** Additive. [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) facet 6 already mints a fresh
`child_session_id` for every real delegation in practice (`DispatchDelegation`
always mints one); this makes that practice an explicit, documented, and
validated invariant rather than an implicit consequence of how the one
existing call site happens to be written.

**Why.** An id conflated across entity types is harmless until the two
id-spaces need to be told apart: a lookup service, a REST path, an audit
query, or a future cascade rule keyed on "is this actually a session id" all
break the moment a tool-call id and a session id can collide or be mistaken
for one another. Crush's own dossier shows the ambiguity is already live, not
hypothetical.

**Cost.** Essentially none: a rule to document and check, not a new field or
event.

### 2. Add a typed "child session purpose" alongside `CascadePolicy`

**The change.** Add an enum (or reuse/extend `OperationKind`) to
`DelegationDispatched`/`ParentLinked` distinguishing *why* a child session was
created, separate from *what happens to it* on parent termination
(`CascadePolicy` already answers the latter).

**Evidence anchor.** Crush (9/12) mints child-session-shaped rows for at
least three distinct purposes through the identical, unenforced mechanism:
a genuine subagent task (`CreateTaskSession`, `internal/session/session.go:110-122`),
a one-off title-generation utility call (`CreateTitleSession`,
`:124-136`), and an `agent-tool` session addressed by the `$$`-composite id
(`CreateAgentToolSessionID`, `:351-368`, invoked from
`internal/agent/coordinator.go:1404-1406`). All three inherit the same
(absent) cascade discipline because nothing distinguishes them at the type
level.

**Blast radius.** Additive: a new enum field, existing consumers ignore it.

**Why.** `CascadePolicy` answers "what happens to the child when the parent
ends," but not "why does this child exist," and those two questions plausibly
want different default answers. An ephemeral, single-completion utility child
(Crush's title-generation case) arguably never needs the same lifecycle
ceremony as a genuine multi-turn subagent; without a typed purpose, that
distinction can only live in caller convention, which is exactly the pattern
that let Crush's three cases silently converge on one unenforced mechanism.

**Cost.** One more field to keep populated correctly at every delegation call
site; unused, it is dead weight on the event.

### 3. Do not add a physical parent-cost rollup field; if usage aggregation is wanted, make it a projection fold

**The change.** Explicitly reject, in the ADR, a written event or mutable
field that rolls a child session's cost/usage up into its parent. If a
"total cost including subagents" view is needed, derive it by folding the
lineage projection (`DelegationDispatched` → child streams →
`OperationOutcomeRecorded`), never by storing a mutated total anywhere.

**Evidence anchor.** Crush (9/12), `internal/agent/coordinator.go:1487-1503`:
`updateParentSessionCost` performs `parentSession.Cost += childSession.Cost`,
an unguarded read-modify-write-and-save, not the atomic
`UPDATE ... SET cost = cost + ?` pattern Crush itself uses elsewhere
(`UpdateSessionTitleAndUsage`, `internal/db/sql/sessions.sql:55-63`). The
dossier flags this as a possible race left unverified, relying entirely on
`SetMaxOpenConns(1)` rather than an atomic increment or a fold.

**Why not to do this.** Any write-side rollup of a child's cost onto the
parent invites exactly Crush's race, two concurrent children completing under
commuting, `Any`-classified facts with no compare-and-swap, and duplicates
data [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) facet 8 already assigns to rebuildable projections. A stored
rollup is a second source of truth for a number a fold can always recompute
correctly.

**Blast radius.** Additive if adopted as "no such field, ever." Breaking the
decision (facet 8) if a future proposal instead adds a written parent-side
usage-rollup event, since that would put a derivable aggregate back into the
log as a second source of truth.

**Cost.** None if simply not built; a projection to maintain if a rollup read
model is later wanted.

### 4. Name the compaction fold as a shared, testable obligation, not an implementation detail of one call site

**The change.** Make explicit, as a documented (and ideally test-enforced)
rule under [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) facet 4/facet 8, that every projection reconstructing
model-visible context must derive it from the same shared "fold from the
newest `Compacted` marker forward" function, rather than each reader
re-implementing the covers_from/covers_through walk independently.

**Evidence anchor.** Crush (9/12), `internal/agent/agent.go:1692-1711`
(`getSessionMessages`, quoted in the stage-one dossier): the truncation
implied by `summary_message_id` is applied at exactly one call site. The
dossier is explicit that "any other reader of `messages.List` (REST
`.../history`, `.../messages`, the CLI, `crush stats`) sees the full,
untruncated row set."

**Why.** This works for Crush today only because there happens to be exactly
one reader that needs the truncated view. The moment a second such reader is
added and does not know to reimplement the same slice logic, it silently sees
more history than intended, an easy, quiet bug. Our `covers_from`/`covers_through`
design is already structurally better (a range on `SessionOrdinal`, not an
index into an in-memory list), but that advantage is only real if every
consumer actually goes through one shared fold rather than each hand-rolling
the "skip to the newest marker" logic the way Crush's single call site does.

**Blast radius.** Additive: an implementation/testing discipline, not a
schema change.

**Cost.** A shared library obligation and a corresponding test, not a wire
cost.

## What our design already does better

**Compaction: a range with provenance, not a bare pointer, applied by policy
rather than by convention.** Crush's marker pattern agrees with ours in the
way that matters most: neither ever deletes or rewrites the durable message
history to compact it. `sessions.summary_message_id` plus
`messages.is_summary_message` (`internal/db/migrations/20250515105448_add_summary_message_id.sql`,
`internal/db/migrations/20250810000000_add_is_summary_message.sql`) is
structurally the same "in-stream marker, read-time truncation" idea as our
`Compacted` event, which is exactly the pattern [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) facet 4 says
"corrects the platform compactor crate, which overwrote the stored message
list wholesale." Where ours is stronger: `covers_from`/`covers_through` are
`SessionOrdinal`-typed *ranges*, stable across restore and migration, versus
Crush's single `summary_message_id`, an index resolved by a linear scan of an
in-memory list (`internal/agent/agent.go:1696-1502`); and `Compacted` records
`trigger`, `guidance`, `tokens_before`/`tokens_after`, `model`, and `usage`,
none of which `is_summary_message` captures at all. As recommendation 4
above notes, Crush's version also shows the risk of leaving the truncation
rule as one call site's implementation detail rather than a decision-level
fold obligation.

**Content-addressed file storage instead of full-content-per-version rows.**
`files.content TEXT NOT NULL` (`internal/db/migrations/20250424200609_initial.sql:28`)
stores the entire file on every recorded version, never diffed, never
deduplicated; the dossier notes "N edits to one file cost roughly N (or 2N)
full copies inside `crush.db`, unbounded by anything but the file's own size
times edit count." This is a second, independent confirmation of the exact
anti-pattern the [fx comparison](../fx/vs-session-events.md) already
flagged (`previous_content` full-pre-image inlining). Our `before_ref`/
`after_ref` `ArtifactRef` pair with `Digest` deduplicates identical content
globally and keeps the event itself small.

**`ResourceObservation` answers what Crush's `read_files` table cannot.**
Crush's freshness sidecar records only `read_at`, a timestamp, per
`(path, session_id)` (`internal/db/sql/read_files.sql:1-11`). It cannot answer
"what did the agent actually see," "how much of the file," or "was it
re-read after an external change," because it carries no digest and no byte
range. Crush has to pay for a coarse version of that last question a
different, more expensive way: `commitFileChange`
(`internal/agent/tools/edit.go:246-269`) inserts an entire extra
"intermediate" content row purely to detect that on-disk content drifted from
the last recorded version. Our `ResourceObservation{content_digest, range,
complete}` on `ToolCallCompleted.observed` gets the same drift signal from a
digest already computed for audit purposes, at no extra storage cost, and
answers the audit and coverage questions Crush's timestamp-only sidecar
cannot.

**Real optimistic concurrency instead of a retry-on-collision loop.** The
dossier confirms, by explicit grep, that there is no expected-version
precondition anywhere in Crush's session, message, or history service layers.
Its closest analog, `history.service.createWithVersion`'s three-attempt
retry on a `UNIQUE` constraint violation (`internal/history/file.go:84-135`),
auto-increments a version the caller never stated an expectation about; it is
retry-on-collision, not a caller-supplied compare-and-swap. Our
`WRITE_PRECONDITION` (`NoStream`/`At`/`Any`) is a real, server-enforced
precondition on every invariant-bearing transition.

**Durable facts instead of a debounce-buffered mutable row.** Crush's
`message.Service.Update` explicitly documents itself as eventually
consistent, buffering state in memory for up to a 33ms debounce window before
it is durable, and requires callers doing a "session-switch read" to call
`Flush`/`FlushAll` first or risk missing the most recent state
(`internal/message/message.go:31-45`, quoted in the dossier). A fact in our
catalog is durable the instant its append acknowledges; there is no class of
"the write technically happened but the read raced it" bug for us to guard
against with a manual flush discipline.

**Typed cancellation, failure, and cascade causes instead of a free-text
`Details` field.** Crush's `Finish` content part carries `Message, Details
string` (`internal/message/content.go:55-146`); our `SessionCancellationReason`,
`SessionFailureReason`, and `ParentTerminalCause` are all typed enums a
projection can switch on directly.

**Real, typed cascade machinery instead of confirmed silent orphaning.** See
the Subagent cascade section below.

## Trade-offs, not gaps

**Real erasure now, versus audit-forever with erasure deferred.** Crush's
`session.service.Delete` performs an actual transactional SQL `DELETE` of a
session's own messages, files, and row: for a targeted session with no
children, that is genuine byte-level erasure, today. Our `SessionHidden` is
explicitly only a visibility tombstone; `ArtifactErased` only removes
out-of-line artifact bytes; erasure-grade deletion (crypto-shredding) is
named in [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) facet 7 as deferred to a follow-up ADR. Crush buys true,
immediate erasure for the single-session case at the cost of zero audit or
rewind history and, as the cascade section below shows, badly broken erasure
semantics the moment children exist. We buy full audit and rewind history and
a cascade design built to avoid Crush's exact failure mode, at the cost of
not yet shipping a real "erase my data" guarantee. Neither is a strict
improvement on the other; a product that must honor a "delete my data" request
today, and cannot wait for the deferred follow-up ADR, would find Crush's
answer closer to what it needs, provided Crush's cascade gap were fixed.

**Database-file-per-project isolation, versus a queryable shared store.**
Crush's project boundary is the `.crush/crush.db` file boundary itself; there
is no `workspace_id` column anywhere, so cross-project leakage is
structurally impossible by construction, not by access-control code. The
cost is visible in `crush stats` (`internal/cmd/stats.go:243-388`), a
bolted-on, read-only, migration-skipping filesystem crawl needed just to
aggregate across projects. Our `WorkspaceRef` on `SessionStarted` makes the
workspace binding queryable inside one shared store, at the cost of workspace
isolation being an access-control and projection concern enforced in code,
not guaranteed by an OS file boundary.

**Same-transaction trigger-maintained counters, versus fold-derived read
models.** Crush's `message_count` is maintained by an `AFTER INSERT`/`AFTER
DELETE` SQL trigger firing in the same transaction as the write it summarizes
(`internal/db/migrations/20250424200609_initial.sql:68-82`), so, unlike a
counter updated by a separate application step, it genuinely cannot drift.
This is not obviously worse than folding for a store built on a single
transactional SQL database. It does not generalize to us: a session's events
and any cross-fact "counter" have no shared transaction to piggyback on in a
one-subject-per-append design, so folding is the only sound option for us,
not merely the more elegant one.

## What not to copy

- **Deriving one entity's id from another entity's id.** Tool-call-id-as-
  session-id (`CreateTaskSession`, `internal/session/session.go:110-122`) and
  the deterministic `"title-" + parentSessionID` composite key
  (`internal/session/session.go:124-136`) both create ambiguous identity
  across what should be separate id-spaces (see recommendation 1).
- **Provider-specific fields folded directly into a canonical struct.**
  `ReasoningContent.ThoughtSignature // Used for google` and
  `ReasoningContent.ToolID // Used for openrouter google models`
  (`internal/message/content.go:55-146`) bake per-provider bleed straight
  into the canonical content type, exactly the leaky-abstraction pattern our
  contained `ProviderBlock`/`ThinkingBlock.signature` escape hatch is
  designed to avoid.
- **Full-content, non-deduplicated version rows for file history.**
  `files.content TEXT NOT NULL` stores whole files on every version with no
  dedup; unbounded growth by construction (see already-does-better).
- **An unenforced, DDL-invisible parent-child relationship as the sole
  cascade mechanism.** The central finding of this comparison; see the gap
  section below.
- **A client-buffered write path whose correctness depends on callers
  remembering to flush.** `message.Service`'s debounce-then-flush contract
  substitutes a manual discipline ("session-switch reads" must call
  `FlushAll`) for a durability guarantee.
- **An unguarded read-modify-write-and-save for a derived numeric rollup.**
  `parentSession.Cost += childSession.Cost` (`internal/agent/coordinator.go:1497`)
  relies entirely on a single global connection lock rather than an atomic
  operation or a fold (see recommendation 3).

## The two gaps the industry has not closed

### Subagent cascade

[ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 6 already takes a detailed position: dispatch is
parent-first with crash-safe reconciler repair (`DelegationDispatched` on the
parent, then an atomic `[SessionStarted, ParentLinked]` batch on the child
under `NoStream`); the graph is acyclic by construction, not by a runtime
check; rewind invalidation (`ParentHistoryInvalidated`) is explicitly
distinct from terminal cascade (`ParentTerminated`, carrying a typed
`ParentTerminalCause`); terminal cascade is driven by a reconciler
[processor](../../../glossary/processor) subscribed to
`session.sessions.events.>`, discovering children through the parent-to-
children lineage projection folded from `DelegationDispatched`, and is
transitive across a chain of depth D in D sequential reconciler round-trips;
`CascadePolicy` makes the child's fate on parent-terminal an explicit,
recorded choice (`CASCADE_ON_PARENT_TERMINAL` default, `INDEPENDENT` for an
intentional, recorded orphan); and a cross-stream atomic delete was
considered and rejected as unavailable ("a single `decide` names exactly one
`StreamId`, and JetStream offers no atomic write across subjects").

Crush's evidence: `parent_session_id TEXT` carries no foreign-key constraint
in any of the 7 migrations (`internal/db/migrations/20250424200609_initial.sql:6`),
unlike `files.session_id`, `messages.session_id`, and `read_files.session_id`,
which all declare `ON DELETE CASCADE` and are genuinely enforced, because
`PRAGMA foreign_keys = "ON"` is set on every connection
(`internal/db/connect.go:19`). `session.service.Delete`
(`internal/session/session.go:138-169`) does not query
`WHERE parent_session_id = ?` and does not walk children. `ListSessions`
filters `WHERE parent_session_id IS NULL` (`internal/db/sql/sessions.sql:40`),
so an orphaned child is not merely unlinked, it is invisible to every normal
listing surface while its rows persist indefinitely. No reconciliation or
orphan-sweep job was found anywhere in the codebase.

**Does this validate, refine, or challenge decision 6?** It validates it,
strongly, and sharpens the argument for it. The cross-product
[synthesis](../../synthesis.md) already observes that "every product that has
subagents has the same unresolved gap" (#7); Crush is a sharper instance of
that gap than most, because it is not simply a case of nobody having
implemented cascade. Crush's schema *does* enforce cascade, correctly, for
every other parent-child relationship in the same database, with the
enforcement pragma switched on. The one relationship that most needed it, a
session's own children, was simply never given a constraint, in a codebase
otherwise disciplined enough to cascade-delete consistently. That is direct
evidence that "we will remember to walk the children" is not a safe design
even for a team careful enough to get every other foreign key right, which is
exactly why decision 6 makes cascade a typed, event-sourced, reconciler-driven
fact (`CascadePolicy` plus a lineage projection) rather than trusting a DDL
constraint or an application-level recursive delete. It also validates
rejecting the cross-stream-transaction alternative on grounds beyond
unavailability: Crush shows that even where a cross-table cascade *is*
trivially available (a plain SQL foreign key, in a single shared database),
it still was not used. A harder-to-forget mechanism, not merely an available
one, is the actual requirement.

Where Crush's answer is worse: total invisibility. An orphan in Crush is
silently unreachable through every normal surface and persists forever with
no sweep; our design's default `CASCADE_POLICY_CASCADE_ON_PARENT_TERMINAL`
actively terminates a child rather than merely making it theoretically
discoverable. One open point this evidence sharpens: [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md)'s own
Consequences section lists "a scheduled orphan-closure sweep" as a "new
standing service" still to be built. Crush is a concrete demonstration that a
*designed* answer with no operational sweep yet running and tested produces
exactly Crush's failure mode until the sweep exists, is running, and is
verified against this exact case, not merely documented as planned.

### Retention on an unbounded log

[ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md) decision 7 already takes a detailed position: the log is never
truncated or purged, full stop; `SessionHidden` replaces the old
`SessionDeleted` as a visibility tombstone with a typed reason
(`SESSION_HIDDEN_REASON_USER_REQUESTED`, `SESSION_HIDDEN_REASON_RETENTION_POLICY`)
that removes a session from default surfaces and still cascades as a terminal
marker, but deletes no bytes; `RedactionApplied` masks targeted events'
content at read time while the original bytes remain, and, because
redelivered duplicates share one deterministic event id, redaction by event
id automatically covers duplicates and every fork's inherited context;
`ArtifactErased` separates out-of-band artifact-byte destruction from
event-log retention; erasure-grade deletion (crypto-shredding) is explicitly
named as deferred to a follow-up ADR, not silently dropped; this explicitly
supersedes [ADR#0029](../../../../adr/0029-decider-retention-and-truncation-watermark.md)'s purge for session streams; and optional, reversible
cold-storage tiering is available if a deployment needs to bound the hot
stream.

Crush's evidence: no retention or TTL policy of any kind was found anywhere
in `internal/`, no scheduled cleanup job, no TTL column; deletion is
exclusively user/CLI/REST-initiated. But when Crush *does* delete, it is a
real transactional SQL `DELETE` of the session's own messages, files, and row
(`internal/session/session.go:138-169`), not a tombstone. Crush has no
in-between state at all: a session is either fully present or fully,
physically gone (for its own rows; see the cascade section for what happens
to children). Growth is genuinely unbounded on the read path: `files.content
TEXT NOT NULL` stores whole file content on every version with no dedup, and
`ListMessagesBySession` has, in the dossier's words, "no pagination, cursor,
or offset anywhere in the read path... cost scales linearly with the number
of messages ever created in that session, no stated bound found." The dossier
does not cite any issue report showing this became a user-visible problem in
practice; that absence is the dossier's own limit (a source read, not an
issue-tracker search), not a claim that no such problem exists, and I am
carrying that uncertainty forward rather than hardening it into "unproven."

**Does this validate, refine, or challenge decision 7?** It is a genuine
trade-off, not a one-sided validation, and the honest reading cuts both ways.
On unbounded growth, Crush adds a third data point supporting the
synthesis's observation (#7) that "the two purest event-sourced designs are
also the two with zero retention story": Crush, while not itself
event-sourced, still ships with no automatic retention at all, so the
absence of a retention story is not unique to event-sourced designs, it is
the industry default regardless of storage model. That part validates
decision 7's premise that retention must be designed deliberately, because
nothing in any of these patterns forces it. On deletion *semantics*
specifically, decision 7 is refined, not validated, by this evidence: for a
single session with no children, Crush's binary hard-delete is a strictly
stronger guarantee, real, immediate, byte-level erasure, than what
`v1alpha1` currently offers, where `SessionHidden` promises only masking and
real erasure is explicitly deferred to a named follow-up ADR. A product that
must honor a "delete my data" request today, and cannot wait for that
follow-up ADR to ship, would find Crush's simpler answer closer to what it
needs for the no-children case, even though Crush's answer collapses
entirely the moment a child session exists (the cascade gap above). The
recommendation this evidence supports is not to abandon keep-forever, [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md)
already gives good reasons in the Alternatives Considered section for
rejecting truncation, but to keep treating the deferred erasure-grade
follow-up ADR as a named, prioritized gap rather than letting "we chose
keep-forever deliberately" read as "erasure is solved."

What Crush does not have, that decision 7 does: no `SessionHidden`-equivalent
distinction between "hidden from listing" and "physically deleted" (deletion
always means physical `DELETE`); no `RedactionApplied`-equivalent partial,
targeted content masking of specific events, only whole-row deletion; no
`ArtifactErased`-equivalent independent artifact-lifecycle, file-version rows
are deleted wholesale with the session, never erasable individually by
artifact id.

## Open questions for the ADR

- Should `DelegationDispatched`/`ParentLinked` carry a typed "purpose" in
  addition to `CascadePolicy`, so an ephemeral utility child (a one-off
  completion, in Crush's case title generation) is distinguishable at the
  type level from a genuine multi-turn subagent, rather than only by
  convention at the call site (recommendation 2)?
- Should the ADR add an explicit, testable invariant that no session id may
  ever be derived from, or reused from, another entity's id (recommendation
  1)?
- Is parent-cost/usage rollup in scope for the session event catalog at all,
  or strictly a downstream projection concern over the lineage plus
  `OperationOutcomeRecorded` usage fields, with no written event ever
  representing it (recommendation 3)?
- Does a user-issued, inline command run directly in the conversation
  (Crush's bang-mode `ShellCommand` content part, distinct from a
  model-requested tool call) need its own representation in our catalog, or
  is it always modeled as an ordinary tool call attributed to a human actor
  rather than the model?
- Should the orphan-closure sweep named in [ADR#0035](../../../../adr/0035-session-store-decider-aggregate.md)'s Consequences section
  carry an explicit test derived directly from Crush's failure mode (an
  orphan invisible to listing yet permanently persisted), given that
  "designed" and "implemented and verified against this exact case" are
  different guarantees?

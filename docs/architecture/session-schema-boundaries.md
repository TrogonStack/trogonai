# Session Schema Boundaries

This page records what is deliberately not in the session schema, and why.

There are two kinds of absence here. Three fields were proposed and declined:
a conversation language, an external work identity, and per-tool telemetry. And
fifteen storage shapes that other session stores adopt are absent by design,
recorded here because a shape that is merely missing looks like an oversight.

A decision not to add something is worth writing down for the same reason an
addition is: otherwise it gets re-proposed every time somebody notices the gap.

See [Session Aggregate](./session-aggregate.md) for the catalog these were
weighed against and [Event Metadata](./event-metadata.md) for the payload and
header boundary two of them turn on.

## The test a proposed field has to pass

An event payload is a canonical domain fact: something replay, a projection, or a
business rule depends on
([Event Metadata](./event-metadata.md)). The question for any proposed field is
therefore narrow, and it is not "is this useful".

**Does a fold rule or an invariant read it?**

If nothing in the aggregate conditions on a value, putting it on an event
commits the domain to carrying it forever, in a log that is never truncated
([ADR#0035](../adr/0035-session-store-decider-aggregate.md) facet 7), for the
benefit of readers that could have gotten it elsewhere.

## Conversation language

The proposal: record the conversation's language, because language selection can
change system prompting, safety behavior, and localization.

The condition is real, and it is already met somewhere else. A session's system
prompt lives inside `StoredSessionExecutionPlan`, which is opaque `plan_bytes`
plus a digest, immutable for the life of the session. If language drives
prompting, it is *already* durable, inside the plan, and it is already covered by
replay because the plan digest is.

Adding a `language` field alongside it produces two fields for one fact. They can
disagree, and the one that disagrees is the one a caller trusted. A session
whose plan was built for one language and whose language field says another is a
session where neither value can be relied on.

If language only drives display, it is a projection concern and belongs in a read
model, where it can change without becoming history.

Nothing in the current catalog conditions durable behavior on a language value.

**When this reverses:** when language becomes a thing that can change mid-session
independently of the execution plan. At that point it is a per-turn fact, not a
session preference, and it should be recorded per turn. A mutable
session-level language would be the worst of the three options: durable enough to
be trusted, mutable enough to be wrong for the turns that came before it.

## External work identity

The proposal: record an external work identity, so an ACP work request that
reconnects resumes under the same identity.

The session already carries identities at every level a fold rule uses: turn id,
message id, tool call id, tool execution id, operation id, and delegation ids.
Above them, the envelope carries correlation and causation headers, and
[Event Metadata](./event-metadata.md) is explicit that correlation is what
headers are for.

An external identity that no invariant reads is correlation. It gets translated
at the boundary, onto the header where correlation lives, and the session's own
identifiers stay the things the aggregate reasons about. Admitting it into the
payload would mean the domain's invariants become sensitive to an identifier
minted by a protocol adapter, which is precisely the coupling the boundary
exists to prevent.

**When this reverses:** when an external identity determines an outcome the
aggregate must enforce. Concretely: if two work requests arriving under the same
external id must be deduplicated by the decider, then the id is a precondition,
and a precondition belongs in the payload. Until then it is metadata that happens
to be interesting.

## Tool-specific telemetry

The proposal: record web fetch and search telemetry, such as cache hit, HTTP
status, response size, and duration.

The mechanism already exists and needs nothing added. A tool returns
`ToolCallResult.artifact_ref`, and every `ArtifactRef` carries a required `mime`.
A web fetch tool can define its own media-typed result shape, publish it, and put
whatever telemetry it wants in there. Nothing in the session domain has to know
what an HTTP status is.

The cost of the alternative is not schema size. It is that the generic session
schema would acquire web semantics, and the next tool would ask for its own
fields on the same grounds, and the catalog would slowly become the union of
every tool that ever shipped.

**The promotion rule**, which is the actual deliverable here: a field moves from a
tool's own result shape into the session catalog when *multiple* tools need it
and read models need its semantics to be stable across them. Duration qualified
under that rule and is already on `ToolCallCompleted`. Cache-hit and HTTP status
do not: one tool needs them, and no projection folds them.

## Shapes weighed and rejected

The three sections above are about fields. These are about designs: ways of
laying out a session store that appear in other systems and are deliberately not
used here. Each is grouped under the property it trades away, because the
property is the reusable part. A future proposal that trades away the same
property is the same decision arriving under a new name.

### Rebuildable state stays rebuildable

**Authoritative counters on a manifest.** History length, token totals, byte
counts, and last-sequence numbers are projection data. Stored as authority they
can drift from the log they summarize, and a summary that disagrees with its
source is worse than no summary, because the reader has no way to tell which one
is lying. They stay derived and rebuildable.

**Derived staleness inside an immutable event.** Whether a file is still current
is a fact about the world that changes after the event is written. Written into
the event it is either wrong later or it forces a rewrite of history to stay
right. Freshness lives in projections, where it is allowed to change.

### The log is append-only and never truncated

**History replacement events.** A `started` / `chunk` / `committed` sequence that
swaps out prior history is appropriate for replacing a local canonical file. It
is not appropriate here, because
[ADR#0035](../adr/0035-session-store-decider-aggregate.md) keeps the
authoritative log append-only and represents compaction as a self-sufficient
marker rather than as a rewrite. Bulk import that needs chunked publication is an
infrastructure import protocol, not a history-rewriting session event.

**Usage checkpoints in the session log.** Sampled logs from stores that do this
are dominated by usage events, and facet 7 means every one of them is kept
forever. Settlement is a downstream ledger with its own retention, which is why
it lives in its own domain. The general form of this rule: anything emitted once
per interval cannot be an event in a log that is never truncated.

### A fact is recorded at the granularity it happened

**Turn-granular history commits.** Committing once per turn loses the in-flight
assistant and tool facts that a crash lands in the middle of, and it forces an
interrupted turn to be represented as one compound record describing several
things that did and did not happen. Facts are committed as they occur.

**A compound interrupted-turn record.** The same problem from the read side. An
interruption is expressible as typed assistant and tool terminal facts plus the
operation ledger, where each part says exactly what it knows. A single record
covering all of it has to encode absence positionally, and absence encoded
positionally is the failure mode described in
[Session Tool Effects](./session-tool-effects.md).

### A binding is immutable, so a change is a new session

**Workspace rebind.** The execution plan and the workspace binding are immutable
for the life of a session. Rebinding means the events before the rebind describe
work in a place the session no longer claims to be. Continue through a new
session or a fork.

**Session-level preference mutation.** What is recorded is the settings each
generation actually used, which is a per-generation fact that stays true. A
mutable session-level preference is durable enough to be trusted and mutable
enough to be wrong for every turn that preceded the change.

### Content goes out of line, addressed by its digest

**Inline file pre-images.** Carrying prior file content inline is the single
largest cost in stores that do it. It bloats events and every checkpoint folded
from them, and it puts the bytes in the one place erasure cannot reach, because
the log is never truncated. Pre-images are claim-checked and content-addressed
like every other payload of unbounded size.

### Nothing assumes a single machine

**Filesystem locks, stat fingerprints, and absolute paths.** These solve
single-machine file ownership. The equivalents here are JetStream write
preconditions, tenant-aware subject resolution, immutable identities, and scoped
object store references.

**Fixed numeric size limits copied from another implementation.** Limits come
from configured policy and from live NATS and object store limits, which are
deployment facts rather than schema facts. What is worth keeping from the other
implementation is the checked arithmetic and the bounded reads, not its
constants.

### Relationships are typed events; indexes are projections

**Relationship truth held outside the session streams.** A parent and child
ownership edge is a domain fact and stays a typed event in the streams that own
it. A relationship index may exist, but only as a projection, so that it can be
discarded and rebuilt without losing the edge.

### The public contract is versioned and strictly valid

**Unversioned or lenient public JSON.** A public projection carries its own
version at the top level, not only on a nested object, and it emits JSON that
survives a strict parser. Reusing an older internal shape as the public one is
the specific failure
[ADR#0058](../adr/0058-session-query-contract-separate-from-projection.md)
exists to prevent.

### Append time is the occurrence time unless the two can differ

**A creation timestamp on `SessionStarted`.** ADR#0035's event-time policy treats
append time as transport metadata and adds a payload occurrence time only when
the external occurrence can differ from the append. Platform session creation
*is* the creation append, so a payload timestamp would be a second field for one
fact. This reverses if imported or externally created sessions acquire a domain
occurrence time distinct from when they were appended.

**A timestamp on every tool result.** The same rule. A locally completed tool
result occurs at its append. An occurrence time is warranted only for a delayed
external result where the two can meaningfully differ, and that case wants its
lifecycle modeled explicitly rather than a timestamp added generically. See
[Session Detached Work](./session-detached-work.md) for the shape that case
actually takes.

## Layout

No protos. That is the point.

## Status

These are decisions, not deferrals. The three declined fields each name the
specific condition that would reverse them. The rejected shapes are grouped by
the property they trade away, so a proposal that trades the same property can be
recognized as the same decision rather than re-argued from scratch.

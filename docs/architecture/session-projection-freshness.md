# Session Projection Freshness

Session queries read projections, and a projection is behind the stream it is
built from. This page documents the contract that makes that lag visible, and
the mechanism a caller uses to read its own writes. It covers the protobuf
definitions that exist today. There is no Rust implementation yet.

See [Session Query Contract](./session-queries.md) for the queries and
[Session Pagination](./session-pagination.md) for cursors, which interact with
freshness in a way worth knowing about.

## The problem

A user redacts a message and the UI immediately re-reads the session. The
projection has not applied the redaction yet, so the response contains the
content that was just ordered destroyed. The response is well formed. Nothing in
it is wrong as far as the projection knows. The user is looking at a screen
that contradicts what the system just told them happened.

Redaction is the sharp version, but the shape is general: a read that is correct
as of the projection can be wrong as of what the caller already knows. The
caller is the only party with enough information to notice, and only if the
server tells it where the projection stands.

Two fields carry the whole contract:

- Every **request** may declare `ReadConsistency`, the freshness it requires.
- Every **response** carries `ProjectionFreshness`, how current the read model
  was when it answered.

This is the same shape the versioning contract uses, and for the same reason:
the caller states its requirement before the server renders, so a mismatch
becomes a typed refusal rather than a plausible wrong answer.

## Reading your own write

`ReadConsistency` has two modes.

`EVENTUAL` is the default and the common case. Serve whatever the projection
has. Freshness still comes back, so a caller that cares can look.

`AT_LEAST` carries a `ConsistencyToken` naming a write the read must reflect,
and a `max_wait` budget. The server answers once the projection has applied that
position, or fails.

Walk the redaction:

1. The caller applies a redaction. The write lands at `SessionOrdinal` 87.
2. The caller re-reads with `mode = AT_LEAST`, `token = {session, 87}`,
   `max_wait = 500ms`.
3. The projection is at 85. The read waits.
4. At 87 the read answers, with `condition = CURRENT`,
   `processed_watermark = 87`, and `consistency.result = SATISFIED_AFTER_WAIT`.

If the projection is still at 85 when the budget runs out, the read fails with
`PROJECTION_UNAVAILABLE` and `REASON_LAGGING`, carrying the freshness it
reached. That last part is what makes the failure actionable: a caller that can
see it got to 85 and needed 87 can retry with a larger budget, while a caller
handed a bare error can only guess.

The wait budget belongs to the caller because only the caller knows whether it
is serving an interactive request. Unset means do not wait at all: answer if the
projection is already there, fail otherwise.

### Why the token is not opaque

Page cursors are authenticated bytes. Consistency tokens are a plain typed
message, and the asymmetry is deliberate.

A cursor names a scan position, so it is an instruction the server follows, and
a forged one is a way to read what the caller should not. A token can only make
a read wait or fail. The worst a forged one achieves is the caller's own bounded
wait followed by a `LAGGING` error, and it discloses nothing. A MAC would buy
nothing here and would cost every caller the ability to debug its own reads.

The token uses a `SessionOrdinal` rather than a stream sequence, for the same
reason a cursor does: it is fold-derived and reproduces identically on replay,
so it survives a projection rebuild. A list read uses one too. A list projection
that consumes many streams still knows how far it has applied each one, and "the
list reflects my change to session X" is the only freshness question a caller
actually has about a list.

## What a response reports

| Field | Meaning |
| --- | --- |
| `condition` | `CURRENT`, `LAGGING`, or `INDETERMINATE` |
| `projection_generation` | Which projection instance answered |
| `processed_watermark` | How far it has applied |
| `processed_at` | Event time of the last applied event |
| `source_high_watermark` | The source head, as most recently observed |
| `source_observed_at` | When that observation was made |
| `consistency` | What the server did about the caller's requirement |

Three things here are easy to get wrong.

**`INDETERMINATE` is not a degraded `CURRENT`.** Observing the source head costs
a round trip the read path does not always pay. Rather than let a server report
a fabricated head, the contract lets it decline, and the condition then says so.
A caller that needs to know it is reading a write it just made states that with
`ReadConsistency`. It does not infer currency from a condition that means the
server did not check.

**`processed_at` is not a consistency boundary.** Event time is not monotonic
across writers, so comparing timestamps to decide whether a write is included
will eventually be wrong. It is there for a human-facing "as of". Compare
watermarks.

**An unset `source_high_watermark` is not a head of zero.** This is the same
rule the query contract already applies to its elision counters: reporting zero
and not reporting are different statements, and a field that cannot express the
difference forces the server to lie.

### Why the condition enum is small

FX-05 lists five projection states. This enum has three, because the other two
are failure-path conditions and already have a home.

A projection that is rebuilding, invalid, or missing either serves nothing, in
which case the caller gets `PROJECTION_UNAVAILABLE` with the matching reason, or
serves a readable prior generation, in which case it is simply `LAGGING`.
Expressing those states in both places would create two ways to say one thing
and invite them to disagree.

That gives a clean rule for rebuilds: a rebuild that keeps the prior generation
readable answers with `LAGGING`, and a destructive one fails with
`REBUILDING`. Either way `projection_generation` changes when the new
generation takes over, which is what invalidates page cursors minted against the
old one.

## When the projection cannot answer

A projection that is lagging still answers; the caller reads `LAGGING` and
decides. A projection that is corrupt, missing, or mid-rebuild cannot answer at
all, and the two options left are both worse than a normal read.

The first is to fail: `QUERY_ERROR_CODE_PROJECTION_UNAVAILABLE` with a reason.
The second is to fold the session's own stream and answer from the log, which is
the authoritative source the projection is a cache of. `AnswerSource` on
`ProjectionFreshness` is what makes the second option expressible. Without it a
replay answer and a projection answer are the same shape, and a caller cannot
tell that it just paid for a fold or that a few of its usual assumptions no
longer hold.

Falling back is admissible only for a read scoped to one session, because only
then is there a bounded stream to fold. `ListSessions` has no such bound: a
fallback there means replaying every stream a caller can see, which is not a
degraded read, it is an outage with extra steps. So a list query under a
projection failure has exactly two honest outcomes, an answer or a refusal, and
the contract says so rather than leaving it to whoever implements the handler
during the incident.

A replay answer reports `CURRENT`, since the fold ran to the head it read at,
and its `projection_generation` is empty. There is no projection instance to
name, and minting a synthetic one would be worse than leaving it blank: a page
cursor pins the generation it was created under, so a synthetic id is a value a
cursor can bind to and nothing can later honor. The consequence is stated on the
enum. A replay answer must not mint a page cursor, so a caller that needs to
paginate through a projection outage waits for the projection rather than
scanning against a fallback that cannot continue.

## Rebuilding is a state, not a silence

`PROJECTION_UNAVAILABLE_REASON_REBUILDING` tells a caller to come back shortly.
On its own that is a claim with no number attached, and it leaves a client
choosing between polling a rebuild that will take four hours and giving up on
one that will take four seconds.

`RebuildProgress` attaches the number: when the rebuild started, the position it
has applied through, and the position it is working toward. The target is
optional, and unset is the honest answer for a rebuild over a stream that is
still being written to. Reporting the current head as the target instead would
produce a percentage that goes backwards, which is worse than no percentage.

Absent progress means the rebuilder could not say. That is deliberately not the
same as a rebuild that has made no progress, and the field is absent rather than
zeroed so the two cannot be confused.

## Interaction with pagination

A paged scan is pinned to a watermark chosen when it opened. So consistency is
honored when the scan opens and **ignored on continuations**: a continuation is
served from the pinned watermark, and waiting for anything newer could not
change what it returns.

That leaves a hole worth closing explicitly. A caller could pass a stricter
requirement on page 2 and believe the page reflects a write it cannot possibly
reflect. A continuation whose requirement exceeds the one the scan was opened
with is therefore `INVALID_ARGUMENT`, not a silently ignored field.

The two contracts cover different halves of the same question, and the redaction
example needs both. `effective_through` says which ordinal the scan covers.
`ProjectionFreshness` says whether the projection had applied the event at that
ordinal when the scan opened. Neither answers for the other. Separately, a
redaction landing mid-scan bumps `privacy_revision` and the next continuation
fails with `STALE_CURSOR` and `PRIVACY_CHANGED`, so an in-flight scan cannot
keep serving pre-redaction content either.

## Not in Session events

None of this is derived from Session events and none of it belongs in one. A
watermark describes the read path's progress, which is not a fact about the
session. Recording it as an event would make the event log depend on the
projections built from it.

`ProjectionFreshness` is query metadata, and the same values are the natural
source for read-path observability.

## Status

Shipped: `projection_freshness.proto` and `read_consistency.proto`, plus
additive fields on all three request/response pairs, `AnswerSource`, and
`RebuildProgress` on `ProjectionUnavailableDetail`. Lint-clean, formatted,
building, and generating Rust bindings reachable at
`trogonai_proto::session::sessions::queries_v1alpha1`.

Not shipped: the projections themselves, watermark tracking, the wait mechanism,
the replay fallback path, the rebuilder that would report progress, and the
command-response shape that hands a caller a `ConsistencyToken` after a
write. That last one is the gap that matters most for the redaction case: the
contract can express the requirement, but nothing yet tells a caller which
ordinal its write landed at.

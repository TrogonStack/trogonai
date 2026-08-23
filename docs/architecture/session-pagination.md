# Session Pagination

`list_sessions` and `get_session_history` are paged, and both page over data
that is being written to while the caller reads it. This page documents the
cursor contract that keeps a multi-page scan from skipping or repeating rows.
It covers the protobuf definitions that exist today. There is no Rust
implementation yet.

See [Session Query Contract](./session-queries.md) for the queries themselves
and [Session Aggregate](./session-aggregate.md) for `SessionOrdinal`, rewind,
and compaction.

## The two failures worth designing against

Paging a growing collection has exactly two ways to go wrong, and they are not
symmetric.

**Repeating a row** is visible. A transcript shows the same turn twice and
someone files a bug.

**Skipping a row** is invisible. The page after it looks perfectly well formed,
and nothing in the response says an item is missing. A caller cannot detect it,
which is why the contract has to prevent it rather than report it.

Two rules do the work:

1. A cursor names the **identity** of the last row delivered, never a count of
   rows delivered.
2. A scan is **pinned** to a boundary chosen when it opened, and never looks
   past it.

Rule 1 is why appends cannot shift a boundary out from under a scan. Rule 2 is
why a scan describes one coherent state of the world rather than a smear across
several.

## Why not an offset

The natural anchor is "I have seen 50 items, give me the next 50." It is also
the one thing that cannot work here.

An offset is an index into a materialized list, so it is only as stable as that
list. In this system the list is a rebuilt read model over an event stream.
Index 50 in one rebuild is not necessarily index 50 in the next, because
rebuilding is allowed to change denormalization, filtering, and grouping. That
is the entire point of a projection being cheap to rebuild.

`SessionOrdinal` has the opposite property. It is assigned by the fold, it is
1-indexed on the session's own stream, and replaying the same events produces
the same ordinal every time. It survives a rebuild because it is derived from
the stream rather than from the read model.

So history cursors carry an ordinal, and the count of items is deliberately
absent from `HistoryScanCursor`.

## History

The scenario the contract exists for: a user scrolls back through older history
while the agent is still appending new turns.

A reverse scan opens with the session at ordinal 400 and a page size of 50.

1. The server pins `anchor_ordinal = 400` and returns ordinals 400 down to 351.
   The cursor records `last_delivered_ordinal = 351`.
2. Twelve turns are appended. The session head is now 412.
3. The client asks for the next page with that cursor. The server resumes
   strictly below 351 and returns 350 down to 301.

The twelve new turns sit above the anchor, so they were never candidates. No
row moved, so no row was skipped or repeated. The scan finishes describing
history as it stood at ordinal 400, which is a state that actually existed.

`GetSessionHistoryResponse.effective_through` carries that anchor and is equal
on every page of one scan. It is the only honest reading of a paged history: a
scan is a snapshot, not a live view.

That has a consequence worth stating plainly. **An in-progress scan is not how
a caller learns about new turns.** Paging further will never surface them.
Live updates come from the session's event stream, and re-anchoring means
starting a new scan.

Direction is fixed when the scan opens. A continuation that arrives with the
other direction is refused rather than reversed, because reversing mid-scan
produces a page sequence with a gap in the middle that neither the client nor
the server can see.

## Sessions

The list side has a harder problem, and it is worth being explicit about why.

History is append-only, so a row's position never changes. A session list is
ordered by recency, and recency is mutable. Consider a list ordered newest
activity first, paged without pinning:

1. Page 1 returns the 50 most recently active sessions. The cursor sits at
   position 50.
2. A session ranked 300th receives a message and jumps to position 1.
3. Page 2 returns positions 51 to 100 of the new ordering.

Every session between the old and new position of that row shifts down by one.
The row now at position 51 was at position 50 and was already delivered, so it
repeats, and one row at the far end falls off the region entirely. Move a few
rows and the scan quietly drops sessions the caller will never see.

Ordering by something immutable, such as creation order, removes the problem and
also removes the ordering people actually want from a session picker. So the
contract keeps recency ordering and pins the scan instead:
`SessionOrderingKey.ordering_value` is the row's ordering value **as of the
scan's pinned watermark**, not as of now.

This puts a real obligation on the list projection: it must be able to
enumerate its ordering as that ordering stood at a past watermark, not only as
it stands now. `CursorValidity.pinned_watermark` is where the scan records
which point it needs, and cursor expiry is what bounds how long the projection
has to keep that ability. This contract cannot assert the property by itself,
and a projection that ignores it will silently reintroduce the skip above.

`ordering_value` alone is not a boundary. Two sessions can share one, and a
boundary that cannot tell them apart has to either re-emit both or drop both.
`session_id` is the tie-breaker, and the full ordering is `ordering_value`
descending then `session_id` ascending.

The selector is part of the cursor. A continuation that changes scope,
workspace, or the archived filter is refused, because a scan whose selection
changed halfway produces a page sequence that matches neither selection and the
caller has no way to notice.

## What a cursor is

A `page_token` is a serialized `CursorEnvelope`. These types are published so
the format is reviewable and versioned, not so clients can read it. A caller
treats a token as opaque bytes and never constructs, parses, or edits one.

| Field | Purpose |
| --- | --- |
| `CursorEnvelope.format_version` | Envelope format, independent of the query `ContractVersion` |
| `CursorEnvelope.payload` | Serialized `PageCursor` |
| `CursorEnvelope.mac` | Authenticates the exact bytes the server emitted |
| `PageCursor.expires_at` | When the pinned scan stops being honored |
| `HistoryScanCursor.anchor_ordinal` | The pinned effective-history head |
| `HistoryScanCursor.last_delivered_ordinal` | Identity boundary for the next page |
| `SessionListScanCursor.selector` | The request shape the scan was opened with |
| `SessionListScanCursor.last_delivered` | Ordering value plus id tie-breaker |
| `CursorValidity` | Everything the cursor binds to besides its position |

The envelope keeps `payload` as `bytes` rather than an inline message because
protobuf serialization is not canonical. A server that decoded a cursor and
re-encoded it to verify the MAC could produce a different byte string than the
one it signed, and the check would fail on a token it issued itself.

### A cursor is a request parameter

A cursor names a scan position and travels through a client. Without
authentication it is an input the caller controls, and a caller that can edit
one can move the anchor, widen the selector, or point a history scan at another
session's stream. The MAC is what makes that infeasible.

Two rules follow, and both are easy to get wrong:

- **A cursor is never authorization.** Every continuation is authorized from the
  caller's own identity as if it were a fresh request. A token is not a
  capability, and one that leaks must not become access.
- **The request is re-checked against the cursor**, not merely trusted. A
  history cursor presented to a different `session_id`, or a list cursor with a
  changed selector, is `MALFORMED_CURSOR`.

## When a cursor stops being valid

`CursorValidity` exists to answer one question: is the sequence this cursor was
cut from still the same sequence? When it is not, the server refuses rather than
serving a page that looks fine.

| Change | Field | `StaleCursorReason` |
| --- | --- | --- |
| Rewind retracted effective history | `effective_history_revision` | `REWOUND` |
| Compaction replaced a span | `effective_history_revision` | `COMPACTED` |
| Redaction or artifact erasure | `privacy_revision` | `PRIVACY_CHANGED` |
| Projection rebuilt or replaced | `projection_generation` | `PROJECTION_REPLACED` |
| Server no longer renders the minted major | `minted_contract` | `CONTRACT_CHANGED` |
| Pinned scan outlived its window | `expires_at` | `EXPIRED` |

Three of these deserve a note.

**Privacy is tracked separately from rewind** even though both change the
effective prefix. It is the binding that must be checked when nothing else
moved: continuing to serve a scan cut before an erasure keeps handing out
content that was ordered destroyed. Collapsing it into the rewind counter makes
that failure depend on an unrelated code path staying correct.

**Compaction is distinguished from rewind** because nothing was lost. The same
history is still there, summarized. A caller can restart the scan without
treating it as data loss, and a caller that saw only `REWOUND` could not tell.
This costs the projection something: it has to know which change advanced
`effective_history_revision`, not just that one did.

**Stale is not malformed.** Stale means the token was genuine and the view
moved, so restarting the scan is the fix. Malformed means the token was never
issued for this request, and retrying is pointless. Merging them would make a
client retry forever on a bug in its own code.

Expiry exists because a pinned scan is a claim on read-model retention. Without
a bound, an abandoned scan pins a watermark indefinitely.

## End of scan

Pagination ends when `next_page_token` is **unset**. An empty page is not the
end signal, because a page can legitimately come back empty while more pages
remain.

This puts the cost where it belongs. The server must know there is nothing
further before it omits the token, which generally means looking one row past
the page it is returning. The alternative, handing out a token that turns out to
yield nothing, makes every caller pay for one extra round trip and makes "is
this list empty" unanswerable without a second call.

For that reason there is no `has_more` field, which is a deliberate departure
from the shape Fx uses. Two signals for one fact can disagree, and a caller then
has to decide which to believe.

## Freshness is a separate question

`CursorValidity.pinned_watermark` and `ListSessionsResponse.pinned_watermark`
are scan anchors, not freshness reports. They say which point the scan is
enumerating. They do not say whether the projection had caught up to the
caller's own writes when it opened, and a caller cannot derive one from the
other. See [Session Projection Freshness](./session-projection-freshness.md).

The two contracts meet in one place: consistency is honored when a scan opens
and ignored on continuations, since a continuation is served from the pinned
watermark and waiting for anything newer could not change what it returns. A
continuation asking for more than the scan was opened with is
`INVALID_ARGUMENT` rather than a silently ignored field.

## Status

Shipped: `page_cursor.proto`, plus additive changes to `query_error.proto`
(`STALE_CURSOR_REASON_EXPIRED`, `STALE_CURSOR_REASON_COMPACTED`),
`list_sessions.proto` (`pinned_watermark`), and `get_session_history.proto`.
Lint-clean, formatted, building, and generating Rust bindings reachable at
`trogonai_proto::session::sessions::queries_v1alpha1`.

Not shipped: cursor minting and verification, the MAC key custody decision, the
list projection's as-of-watermark ordering index, and the retention window that
expiry is derived from. The last two are the load-bearing ones: the guarantee on
this page is only as true as the projection that has to honor a pinned
watermark.

The same four coordinates `CursorValidity` pins are what a client-held
presentation cache binds to, because the events that invalidate a cursor
invalidate a cache. See
[Session Presentation Caches](./session-presentation-cache.md) for why the two
are separate types anyway.

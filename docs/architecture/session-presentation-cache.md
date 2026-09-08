# Session Presentation Caches

A client that reopens a session wants to paint the timeline before its first
query returns. It usually can: it painted the same timeline an hour ago and
still has the rendered result. This page documents the contract that says
whether painting it again is safe. It covers the protobuf definitions that exist
today. There is no Rust implementation yet.

See [Session Projection Freshness](./session-projection-freshness.md) for what a
projection reports about itself, and [Session Pagination](./session-pagination.md)
for cursors, which are invalidated by the same events for the same reasons.

## The cache is not stale, it is unbound

The instinct is to treat a client cache as a time problem: keep it for five
minutes, then refetch. That framing is wrong in both directions. A session
nobody has touched in a month has a cache that is perfectly good after five
minutes and after five weeks. A session someone redacted thirty seconds ago has
a cache that was already unsafe before the timer started.

Age is not the property that matters. What matters is whether the projection
that produced the cached content still exists and still says the same things, so
the cache carries the projection state it was rendered against and the question
becomes a comparison rather than an estimate.

`PresentationCacheBinding` is that state: the projection generation, the
processed watermark, the effective history revision, and the privacy revision,
plus the contract version and the client's own renderer version.

## The same four coordinates as a cursor, deliberately not shared

Those four projection coordinates are exactly what `CursorValidity` pins for a
page cursor. That is not duplication by oversight. The events that invalidate a
cursor and the events that invalidate a cache are the same events, so the
coordinates have to be the same coordinates.

They stay separate types because they are checked by different parties. A cursor
is minted by the server, opaque to the client, and validated by the server on
the next page. A binding is held by the client, readable by the client, and
checked before anything is painted. Sharing one message would mean a field added
for the server's validator lands in the client's cache format, and a change made
for the client's benefit lands in an opaque token clients are told never to
parse.

## Older is not the same as a prefix

The useful case is a cache at a lower watermark than the current projection.
The client paints what it has and reads forward from there, which is the only
arrangement that shows something before the first query returns.

That is only correct when history between the two watermarks was append-only. It
usually was. It is not after a rewind, a compaction, a redaction, or an artifact
erasure, because all four *retract* content that was there. After any of them a
lower watermark is not a prefix of a higher one, and the cached rows include
rows that no longer exist.

So `CACHE_USABILITY_APPENDABLE` is gated on the two revisions matching, not on
the watermark being lower. A lower watermark alone does not establish that the
interval only added.

The redaction case is the one worth stating plainly, because it is not a
staleness annoyance. A cache painted after a redaction puts content someone
deliberately destroyed back on a screen, in front of a person. That is the
failure the redaction was performed to prevent, arriving through the code path
added to make the app feel fast.

`privacy_revision` is tracked apart from `effective_history_revision` for that
reason: it is the one that must be checked even when nothing else moved.

## The verdicts

`CacheUsability` is a single value rather than a set of flags, so a client
cannot read the half it likes.

| Value | Meaning |
| --- | --- |
| `UNSPECIFIED` | Unknown verdict. Discard. |
| `CURRENT` | Every coordinate matches. Paint as final. |
| `APPENDABLE` | Strict prefix. Paint, then read forward from the cached watermark. |
| `RETRACTED` | Content the cache holds no longer exists. Discard all of it. |
| `REPLACED` | Different projection generation. Not old output, foreign output. |
| `INCOMPATIBLE` | Contract major or renderer version differs. |
| `INDETERMINATE` | The server could not decide. |

The zero value discards, so a client reading a variant added after it was built
falls back to fetching rather than to painting.

`RETRACTED` and `REPLACED` are separate because they are different problems. In
the first the projection is fine and history changed underneath it; in the
second the projection is gone and the content came from a computation that no
longer runs. `RETRACTED` also has no partial answer: the client cannot know
which of its rows were the retracted ones, so it discards all of them.

`INDETERMINATE` exists so a degraded server can say it does not know instead of
resolving to a discard. Resolving to a discard would turn a projection outage
into a refetch storm the same outage cannot serve.

## Checking is not reading

`CheckPresentationCache` is not a read. A caller that wanted the content would
call `GetSessionHistory`; this is for the caller that already has content and
needs a verdict more cheaply than fetching it again. There is no field on the
request that could make it return timeline data, so it cannot quietly grow into
the expensive path it was added to avoid.

The response still carries `current`, the binding a fresh render would use, on
every verdict including a discard. A client about to refetch already knows what
to bind the replacement to, without a second round trip.

## Rendering state stays in the client

Nothing here describes the rendering. Terminal width, scroll position, theme,
and ANSI handling are the client's state and stay there. The event log records
what happened, not what a viewer looked like while watching it, and a projection
that stored presentation state would have to be rebuilt every time a renderer
changed.

`renderer_version` is the one concession, and the server never interprets it and
never compares it. It is carried so a client shipping a new renderer can discard
its own caches without the server knowing anything about renderers.

## Layout

`proto/trogonai/session/sessions/queries/v1alpha1/presentation_cache.proto`:

| Message | Contents |
| --- | --- |
| `PresentationCacheBinding` | the projection state a cache was rendered against |
| `CheckPresentationCacheRequest` | session, contract, held binding |
| `CheckPresentationCacheResponse` | verdict, current binding, freshness |
| `CacheUsability` | what the client may do with what it holds |

No `service` definitions, matching the rest of this repo: transport binding is
JSON-RPC over NATS ([ADR#0055](../adr/0055-nats-subject-design-jsonrpc-bindings.md),
[ADR#0056](../adr/0056-canonical-jsonrpc-bodies-over-nats.md)).

A session's derived title and preview invalidate against the same two revisions
this binding does, for the same reason; see
[Session Title and Preview](./session-title-and-preview.md).

## Status

Shipped: `presentation_cache.proto`, lint-clean, formatted, building, and
generating Rust bindings reachable at
`trogonai_proto::session::sessions::queries_v1alpha1`.

Not shipped: the projection that would track `effective_history_revision` and
`privacy_revision`, the handler, and the client cache itself. Until the two
revision counters are actually maintained, the only honest verdict a server
could return is `INDETERMINATE`.

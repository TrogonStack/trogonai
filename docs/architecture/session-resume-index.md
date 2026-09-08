# Session Resume Index

A resume command needs one session: the newest one in a workspace that it is
actually allowed to attach to. This page documents the query contract for that
answer. It covers the protobuf definitions that exist today. There is no Rust
implementation yet.

See [Session Query Contract](./session-queries.md) for the list and detail
reads, and [Session Projection Freshness](./session-projection-freshness.md) for
what a response says about how current it is.

## Filtering after the list is the wrong order

The available move today is to call `ListSessions` ordered newest first and take
the first row that qualifies. It works until it does not. The list is ordered by
recency and filtered afterwards, so a workspace whose forty most recent sessions
are all running returns a page with no answer in it and no way to tell that a
longer page would have found one. The caller either pages until it finds
something or gives up, and both are decisions it is making blind.

Selection has to happen in the index. `GetLatestSession` asks the question
directly and gets a single session or nothing.

This is also where staleness costs the most. A stale list is a list with an old
row in it, which a human reading a picker can see and ignore. A stale
latest-pointer is the one answer the caller acts on, so the entire error lands
on whichever session gets opened.

## The selector is an enum, not a set of filters

Booleans let a caller ask for a session that is both currently active and not
currently active, and then the server has to decide what an impossible request
means. `SessionSelector` has three values, each a question someone actually
asks:

- `MOST_RECENT`: the newest non-archived session, whatever state it is in.
- `RESUMABLE`: the newest one a resume can attach to.
- `ACTIVE`: the newest one currently being driven, for reattaching.

The zero value is refused rather than defaulted. There is no default that is
safe for both callers: a picker wants the most recent session of any kind, a
resume wants a resumable one, and guessing wrong hands a resume command a
session that is already running somewhere else.

Terminal is not one of `RESUMABLE`'s exclusions. A closed session is the
ordinary thing to resume; that is what resuming means. What is excluded is a
session already being driven from elsewhere, which resuming would turn into two
writers on one stream.

## Empty is defined on authored content

"Exclude empty sessions" needs a definition, and `effective_length` is not it. A
session can carry configuration and lifecycle records and still be one nobody
ever said anything in. A rewind to the start empties a session that was not
empty an hour ago. Redaction can mask an ordinal without removing it.

So emptiness is defined as no user-authored turn in effective history. Resuming
a session that fails that test puts a person back into a blank window they have
no memory of leaving.

## Latest by which clock

`RecencyBasis` picks between last activity and creation, because the two
orderings disagree often. A long session started yesterday and worked on this
morning is the newest by activity and among the oldest by creation.

Both timestamps behind it are recorded external occurrences, not positions
derived from append order (D10), so a projection that catches up late does not
reorder the answer.

## An empty answer has to say why

Absence is a normal outcome and is returned as an unset field rather than an
error, because a first-run workspace is not a failure.

But "no resumable session" and "eleven sessions, all still running" are the same
absence to a caller that only sees an empty field, and they call for opposite
next moves: create a new session, or attach to a running one. `ExclusionCounts`
is what distinguishes them. It reports `considered` as the denominator, so zero
there means an empty workspace rather than an over-strict selector, and then one
count per rule.

A session removed by more than one rule is counted once, under the first rule
that removed it, in declaration order. Counting it under every matching rule
would give totals larger than the workspace, and the number a caller wants is
how many sessions there were, not how many judgements were made.

`unrenderable` is the count that changes the meaning of the whole response.
Non-zero means the answer may be wrong rather than merely empty, because the
session that should have won might be one of the rows the projection could not
render. That is the same rule `ListSessionsResponse.skipped_count` follows, and
it matters more here: a list with a missing row is incomplete, and a
latest-pointer with a missing row is incorrect.

## The index is disposable

Everything this query reads is a projection, rebuildable from the stream, so it
carries `ProjectionFreshness` like every other read and a caller can require
consistency when it needs to. Nothing about the index is authoritative, and
nothing about it belongs in an event: which session is newest is a fact derived
from the sessions, not a fact about any one of them.

## Layout

`proto/trogonai/session/sessions/queries/v1alpha1/latest_session.proto`:

| Message | Contents |
| --- | --- |
| `GetLatestSessionRequest` | workspace, selector, recency basis, consistency |
| `GetLatestSessionResponse` | the session or nothing, exclusions, freshness |
| `SessionSelector` | which session counts as the answer |
| `RecencyBasis` | which clock "latest" is measured by |
| `ExclusionCounts` | what each rule removed, and the denominator |

No `service` definitions, matching the rest of this repo: transport binding is
JSON-RPC over NATS ([ADR#0055](../adr/0055-nats-subject-design-jsonrpc-bindings.md),
[ADR#0056](../adr/0056-canonical-jsonrpc-bodies-over-nats.md)). Query naming
follows verb + noun per [ADR#0014](../adr/0014-command-and-query-naming.md).

## Status

Shipped: `latest_session.proto`, lint-clean, formatted, building, and generating
Rust bindings reachable at
`trogonai_proto::session::sessions::queries_v1alpha1`.

Not shipped: the index itself, the handler, and the notion of a session being
currently driven, which nothing in the aggregate tracks yet. `SESSION_SELECTOR_ACTIVE`
and the `active` exclusion count are contract ahead of mechanism, defined now
because the semantics are the part that has to be settled before anything
indexes against them.

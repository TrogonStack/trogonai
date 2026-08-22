---
number: "0058"
slug: session-query-contract-separate-from-projection
status: draft
date: 2026-08-22
---

# ADR#0058: The Session Read Contract Is a Query Proto, Not the Projection Value

## Context

[ADR#0035](./0035-session-store-decider-aggregate.md) facet 8 settles that no
read model is authoritative: listing, search, and summaries are rebuildable
projections folded from the log, checkpointed by stream position, and discardable
at any time. That part is not in question and this ADR does not touch it.

One sentence inside that facet goes further. It says queries are `verb + noun`
Rust functions over the KV projection
([ADR#0014](./0014-command-and-query-naming.md)) "with no query protos, since the
projection value is the read contract."

That sentence is wrong, and the Session query work on branch
`yordis/study-vercel-fx` contradicts it directly: twelve protos now exist under
`proto/trogonai/session/sessions/queries/v1alpha1`, defining request and response
shapes, a typed error taxonomy, an opaque page cursor, freshness metadata, and
read-model value types redefined locally rather than imported from the write
side.

An accepted ADR should not be contradicted by a merged branch without a
correction on the record, and the correction is not a documentation page. It is a
reversal of a decision.

## Decision

### 1. The projection value is not the read contract

A projection and a public contract have opposite obligations, and no single type
can carry both.

A projection exists to be cheaply rebuilt. Its value type should change whenever
the fold changes, whenever a new field is worth denormalizing, whenever a query
turns out to need something it did not before. The cost of changing it is one
rebuild, which facet 8 already budgets for. That cheapness is the whole point of
the design.

A public contract exists to hold still. Its cost of change is every client that
decoded the old shape, and those clients are not in this repository and do not
deploy on this schedule.

Making the projection value the contract couples the two. Either the projection
stops changing freely, which removes the property that made it worth rebuilding,
or the contract changes at the projection's cadence, which means it is not a
contract. Facet 8's own framing is the argument against its own sentence: a type
described as discardable cannot also be the thing callers depend on.

### 2. Read-model value types are redefined, never shared with the write side

The queries package defines its own `SessionSummary`, `SessionLifecycle`,
`TerminalReason`, `TitleSource`, and the rest, rather than importing the event or
state types.

This is facet 3's sibling-subtree reasoning applied to the read side. The
write-side types change whenever the domain changes, which is often and by
design. A shared type would forward every write-side edit straight to every
client, which is exactly the coupling the local redefinition exists to prevent.
The duplication is the feature: two shapes that are allowed to disagree, with an
explicit mapping between them, beats one shape that cannot evolve on either
schedule.

### 3. The contract is versioned, and the server clamps rather than reports

Each request carries the caller's `accepted_contract`. The server renders at no
higher than the caller's minor and elides what the caller cannot decode, rather
than emitting a variant that fails to parse.

A version only on the response lets a client detect an incompatibility after it
has already failed. That difference matters more here than usual because the repo
generates with `unknown_fields=false`, so unknown fields are dropped on decode
rather than preserved. Nothing round-trips through an older decoder intact, and a
declare-then-clamp handshake is what replaces the tolerance that unknown-field
preservation would otherwise have provided.

### 4. Queries keep `verb + noun` naming and stay Rust functions

Nothing about [ADR#0014](./0014-command-and-query-naming.md) changes.
`get_session`, `list_sessions`, and `get_session_history` are still the names, and
the handlers are still Rust functions reading a KV projection. What changes is
that their inputs and outputs are declared types rather than whatever struct the
projector happened to write.

There are no `service` definitions, matching the rest of this repository:
transport binding is JSON-RPC over NATS
([ADR#0055](./0055-nats-subject-design-jsonrpc-bindings.md),
[ADR#0056](./0056-canonical-jsonrpc-bodies-over-nats.md)). The protos are body
types only, so this decision adds a contract without adding a transport.

### 5. Facet 8's remaining substance is unchanged

No read model is authoritative. Projections are folded from the log,
checkpointed by stream position, and rebuildable. The context compiler is a
projection. Search is a separate projection. Resume loads a snapshot and replays
the tail. All of that stands exactly as accepted.

Only the clause "with no query protos, since the projection value is the read
contract" is withdrawn.

## Alternatives Considered

### Keep facet 8 as written and expose the projection value

This is the accepted position, and it is workable right up to the first external
consumer. It fails at the moment a projection needs a field the contract should
not have, or the contract needs a guarantee the projection cannot give, and both
happen early. Freshness is the clearest case: a caller has to be able to tell a
current answer from a lagging one, and that is a fact about the read *path*, not
a field the fold would ever produce.

### Publish the projection value and version it in place

Versioning the projection value gives the contract stability without a second
type, at the cost of making every projection change a compatibility event. That
inverts the economics facet 8 established. A rebuildable read model whose shape is
frozen by external clients is no longer rebuildable in the sense that mattered.

### Define the contract in Rust rather than protobuf

A Rust type with serde would serve in-process callers. It would not serve a
cross-language client, it would not participate in `buf breaking`, and it would
put the compatibility rules in review comments rather than in a checkable
artifact. The repository already generates every other boundary from protobuf,
and a read contract is a boundary.

### Amend facet 8 in place rather than by a new ADR

Editing the sentence out of an accepted ADR would leave no record that the
decision was reversed or why, which is the failure ADRs exist to prevent. The
reciprocal amendment on facet 8 points here, the way facet 7's points to
[ADR#0057](./0057-session-stream-incarnation-fencing.md).

## Consequences

- Query protos are additive to facet 8, not a replacement for it. The projection
  layer it describes is still what the handlers read.
- The queries package can evolve on its own schedule under `buf breaking`, and a
  projection change no longer implies a client change.
- Two shapes now describe overlapping facts, and the mapping between them is
  code someone has to maintain. That is the accepted cost, and it is smaller than
  the coupling it removes.
- `v1alpha1` is honest and stays there: the contract cannot promote to `v1`
  before the projections it reads exist.
- The numeric JSON-RPC error-code reservation for the query error taxonomy is
  still open, and is reserved by decision rather than invented in a proto file, as
  [ADR#0017](./0017-aauth-agent-authentication.md) did for `-32118`.
- Nothing in this ADR is implemented. The protos exist; the handlers and the
  projections they read do not.

## References

- [ADR#0014: Command and Query Naming](./0014-command-and-query-naming.md)
- [ADR#0017: AAuth Agent Authentication](./0017-aauth-agent-authentication.md)
- [ADR#0035: Session Store as a Decider Aggregate on NATS JetStream](./0035-session-store-decider-aggregate.md)
- [ADR#0055: NATS Subject Design and JSON-RPC Bindings](./0055-nats-subject-design-jsonrpc-bindings.md)
- [ADR#0056: Canonical JSON-RPC Bodies over NATS](./0056-canonical-jsonrpc-bodies-over-nats.md)
- [ADR#0057: Stream Incarnation Fencing by Subject Isolation and Sealing](./0057-session-stream-incarnation-fencing.md)
- [Session Query Contract](../architecture/session-queries.md)
- [Session Pagination](../architecture/session-pagination.md)
- [Session Projection Freshness](../architecture/session-projection-freshness.md)

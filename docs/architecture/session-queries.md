# Session Query Contract

The Session query contract is the public read surface of the Session Store:
`get_session`, `list_sessions`, and `get_session_history`, each with an explicit
version and a typed error taxonomy. This page documents the protobuf contract
that exists today. There is no Rust implementation yet.

See [Session Aggregate](./session-aggregate.md) for the write side. The two are
deliberately separate contracts, and the reason is the whole point of this page.

## Why a query proto exists at all

[ADR#0035](../adr/0035-session-store-decider-aggregate.md) facet 8 says queries
have "no query protos, since the projection value is the read contract." This
contract departs from that, and the departure should be reviewed rather than
assumed.

The problem with a projection value as the public contract is that it makes one
type serve two masters. A projection exists to be rebuilt: it changes whenever
the read model needs different denormalization, and rebuilding it is cheap
precisely because nothing outside the service depends on its shape. A public
read contract exists to hold still: clients compile against it and ship on their
own schedule. Making them the same type means every read-model change is a
client-visible change, and the read model stops being cheap to rebuild.

So the query types here are defined in their own package and redefine their
value types locally rather than importing write-side or projection types. That
is the same rule ADR#0035 facet 3 already applies to the `state`, `projections`,
and `checkpoints` subtrees, extended one step further out.

That departure is now on the record rather than on this page.
[ADR#0058](../adr/0058-session-query-contract-separate-from-projection.md)
withdraws facet 8's clause and states the reversal, and facet 8 carries the
reciprocal amendment. Everything else in facet 8 stands: no read model is
authoritative, projections are folded and checkpointed from the log, and these
handlers are still `verb + noun` Rust functions reading a KV projection.

## Layout

`proto/trogonai/session/sessions/queries/v1alpha1/`:

| File | Contents |
| --- | --- |
| `contract_version.proto` | `ContractVersion`, `ContractNegotiation` |
| `query_error.proto` | `QueryError`, `QueryErrorCode`, typed details |
| `session_view.proto` | `SessionView`, `SessionSummary`, lifecycle enums |
| `history_item.proto` | `HistoryItem` and its variants |
| `get_session.proto` | request/response pair |
| `list_sessions.proto` | request/response pair |
| `get_session_history.proto` | request/response pair |
| `page_cursor.proto` | cursor envelope and scan positions |
| `read_consistency.proto` | the freshness a caller requires |
| `projection_freshness.proto` | the freshness a response reports |
| `presentation_cache.proto` | binding and verdict for a client-held cache |
| `latest_session.proto` | the single newest eligible session in a workspace |

There are no `service` definitions, matching the rest of this repo: transport
binding is JSON-RPC over NATS ([ADR#0055](../adr/0055-nats-subject-design-jsonrpc-bindings.md),
[ADR#0056](../adr/0056-canonical-jsonrpc-bodies-over-nats.md)), and these are the
body types. Query naming follows verb + noun per
[ADR#0014](../adr/0014-command-and-query-naming.md).

## Versioning

Two fields carry the whole scheme:

- Every **request** declares `accepted_contract`, the highest version the caller
  understands.
- Every **response** carries `contract`, holding both the version it was
  `rendered` at and the server's `server_max`.

The compatibility rule is ordinary: same `major` is compatible, different
`major` is not. `minor` increments only for additive changes (a new optional
field, a new enum value, a new union variant); `major` increments for anything a
caller built against the prior version could misread.

What makes it work is that the caller declares its version *before* the server
renders anything.

### Clamping, not just detecting

A version field that only appears on the response lets a client detect an
incompatibility after it has already failed to parse something. This contract
goes further: **the server renders at no higher than the caller's declared
minor.** A caller never receives a variant it cannot decode.

Walk the example. A client is built against `1.2`. The server ships `1.4`, which
added a new history item variant.

1. Client sends `accepted_contract = {major: 1, minor: 2}`.
2. Server sees a matching major, so the request is serviceable.
3. Server renders history. An item that needs the `1.4` variant cannot be
   expressed at `1.2`, so it is emitted as `HISTORY_ITEM_KIND_ELIDED` with
   `reason = ELISION_REASON_CONTRACT_CLAMPED`.
4. Response carries `rendered = 1.2`, `server_max = 1.4`, and
   `contract_clamped_count = 1`.

The client now knows three separate things it could not otherwise distinguish:
the answer is readable, one item was withheld, and upgrading would reveal it. If
instead the client had sent `major: 2`, the server would refuse with
`QUERY_ERROR_CODE_UNSUPPORTED_CONTRACT_VERSION` carrying the supported major
range, rather than answer in a shape the client cannot read.

### Why the discriminated union needs help

A `oneof` cannot express "a variant you do not know." A client decoding an arm
added after its version sees an unset `oneof`, indistinguishable from a variant
that was never set. Silently rendering nothing for a real event is a correctness
failure.

`HistoryItem` closes this two ways:

- `kind` is a plain enum **outside** the `oneof`. Proto enums are open, so an
  unrecognized kind survives decoding as its raw number. A client can always
  tell something is there.
- Elision is **affirmative**. `HISTORY_ITEM_KIND_ELIDED` with a reason is a
  positive statement that an item was withheld, not an absence the client has to
  infer.

`item_id` stays stable across an elision, so a client that upgrades sees the same
item resolve to a real variant rather than a new entry appearing.

One codegen constraint worth knowing: this repo generates with
`unknown_fields=false`, so unknown fields are **dropped** on decode, not
preserved. A relay cannot round-trip a newer response through an older
decoder without loss. The clamping rule is what keeps that from mattering.

## Errors

`QueryError` is not a variant inside a success response. A query either answers
or fails, and merging the two invites a client to read a zero-valued success
shape as an empty result.

`code` is the only field to branch on. `message` is for humans, non-contractual,
and may change without a version bump.

| Code | Meaning |
| --- | --- |
| `SESSION_NOT_FOUND` | No such stream |
| `UNSUPPORTED_CONTRACT_VERSION` | Major mismatch; carries the supported range |
| `INVALID_ARGUMENT` | Malformed input; carries a field path |
| `STALE_CURSOR` | Cursor was genuine, the view moved; carries a reason |
| `MALFORMED_CURSOR` | Cursor was not issued for this request, or fails its MAC |
| `PROJECTION_UNAVAILABLE` | Read cannot be served now; carries a reason |
| `PERMISSION_DENIED` | Not authorized |
| `RESOURCE_EXHAUSTED` | Valid but too expensive as asked |
| `INTERNAL` | Unexpected failure, no detail by design |

Three distinctions here exist because collapsing them loses information a caller
needs to act:

- **Stale vs malformed cursor.** Stale means restart the scan; malformed means
  fix the client. Retrying a malformed cursor is pointless.
- **Not-found vs permission-denied.** Where disclosing the difference would let
  a caller prove a session exists, the server returns `PERMISSION_DENIED` for
  both. Otherwise the error code becomes an existence oracle.
- **Rebuilding vs invalid projection.** `ProjectionUnavailableDetail` separates
  "come back shortly" from "this is broken." A caller cannot guess which from the
  code alone, and the retry behavior differs.

The numeric JSON-RPC code assignment is **not** made here. That space is
reserved by decision, as [ADR#0017](../adr/0017-aauth-agent-authentication.md)
did for `-32118`, and inventing numbers in a proto file would bypass it.

## Completeness is reported, never implied

A response that parses is not evidence that it is complete. Three counters say
otherwise, and a client that ignores them will present partial data as whole:

- `ListSessionsResponse.skipped_count`: rows the server could not decode or
  render. Non-zero means the list is not exhaustive.
- `GetSessionHistoryResponse.contract_clamped_count`: items withheld because the
  caller's version is too old. Upgrading reveals them.
- `GetSessionHistoryResponse.withheld_count`: items withheld by redaction or
  authorization. Upgrading does **not** reveal them, so a client should not
  prompt for one.

`SessionSummary.recovery` is the same rule applied to provenance rather than
completeness. A session salvaged from a damaged one is missing content the
original had, and a list row that renders it identically to an intact session
is where a user decides it is the session they were looking for. See
[Session Maintenance](./session-maintenance.md).

The same principle drives `SessionView.has_unreconciled_work`. A session can
reach a terminal lifecycle while a tool call it started never recorded an
outcome. A reader that treats terminal as complete will show a finished session
that still has work stranded in the ledger, so the fact is surfaced explicitly.

`SessionView.artifacts` extends it to content the session produced but no longer
holds. Artifact bytes live outside the log and can be erased or lost without the
session changing at all, so retrievability is reported separately from lifecycle
rather than inferred from it. See [Session Artifacts](./session-artifacts.md).

End-of-scan follows the same rule: pagination ends when `next_page_token` is
**unset**, not when a page comes back empty. A page can legitimately be empty
while more pages remain.

## Pagination

`page_token` and `next_page_token` are opaque `bytes` carrying a serialized
`CursorEnvelope`. What a token binds to, how a scan stays stable while the
session is still being written to, and exactly when a cursor goes stale are
documented in [Session Pagination](./session-pagination.md).

That contract landed after this one and demonstrates the versioning rule
working as intended: it added `page_cursor.proto`, two `StaleCursorReason`
values, and one response field, all additive, so a client built against the
prior minor keeps working unchanged.

## Freshness

Every request may declare a `ReadConsistency` and every response carries a
`ProjectionFreshness`, so a caller can tell whether the view it received
reflects a write it just made. See
[Session Projection Freshness](./session-projection-freshness.md).

Like pagination, that contract landed after this one and arrived entirely as
additive fields: two new protos, new fields on all three request/response pairs,
and one new field on `ProjectionUnavailableDetail`. Nothing a client built
against the prior minor had to change.

## Beyond the three reads

Two contracts sit alongside these and are documented on their own pages. A
client that reopens a session can check a cache it already holds instead of
refetching it, which is
[Session Presentation Caches](./session-presentation-cache.md). A resume command
needs the one newest eligible session in a workspace rather than a page it has
to filter, which is [Session Resume Index](./session-resume-index.md).

How a row's title and excerpt are resolved, and what their absence means, is
[Session Title and Preview](./session-title-and-preview.md).

## Status

Shipped: the twelve protos above, lint-clean, formatted, building, and generating
Rust bindings reachable at `trogonai_proto::session::sessions::queries_v1alpha1`.

Not shipped: query handlers, the projections they read, the transport binding,
and the numeric JSON-RPC error-code reservation.

`v1alpha1` is honest: this contract cannot be promoted to `v1` before the
projections it reads exist.

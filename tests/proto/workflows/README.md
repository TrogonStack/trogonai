# Workflow protobuf validation fixtures

The fixture runner builds the current workflow descriptors and checks each payload with the repository's pinned Buf and `buf convert --validate`. Fixture keys are checked against the descriptors before conversion because Buf's JSON reader can discard unknown fields. A parser failure never counts as a successful validation rejection.

## Command reference

```sh
mise run proto:validate-workflows
mise run proto:validate-workflows -- --case query-history
```

`--case` selects case names by substring. `--buf` overrides the executable; its version must still match the repository pin. The runner uses Python's standard library and the installed Buf executable.

## Fixture format

Each JSON file defines typed payload templates and named cases. A case can apply `add`, `replace`, or `remove` JSON Pointer patches to its template. A case without `violations` must pass message-local validation. A case with `violations` must parse successfully and fail Protovalidate with every listed diagnostic.

| Fixture | Contract coverage |
| --- | --- |
| `values-policies.json` | Typed values, declared schemas, scoped references, conditions, retry policies, limits, and digests |
| `definitions.json` | Node identity and ownership, references, port bindings, joins, repeats, input waits, and delays |
| `execution.json` | Iteration and attempt identities, Session launch admission, correlated result receipts, and pinned run plans |
| `control-lifecycle.json` | Completion and business rejection, sequence and join decisions, repeat bounds, pauses, cancellation intent, plan revisions, and failed-work recovery |
| `waits.json` | Watcher registration, activity and cursor receipts, activation, lease renewal, cleanup, and timers |
| `state.json` | Retained definitions, matched receipts, answers, wait progress, pause boundaries, cancellation acknowledgements, plan snapshots, and terminal evidence |
| `settlement.json` | Producing Session identity, terminal effect settlement, responder audiences, dispatch identities, and reconciliation |
| `commands-events.json` | Command and event envelopes with nested contract constraints |
| `queries.json` | Contract negotiation, consistency requests, paging, history, launch rejection, and projection error progress |
| `definition-queries.json` | Saved-definition selection, exact artifact references, consistency, and projection freshness |
| `cursors.json` | Page scope, snapshot bounds, caller binding, query discrimination, and freshness |
| `plans.json` | Exact ad hoc and revised plan artifacts, lineage, and plan query selection |

## Evidence boundary

Passing fixtures establish descriptor parsing and message-local constraints. They do not establish that a plan has been admitted, an event is authentic, a transition is legal for prior history, or a scheduler executes the workflow correctly. Opaque definition bytes, digests, and receipt references in these fixtures are placeholders. Admission must verify their actual content and provenance.

The cases `schema-admits-disconnected-cycle-requiring-semantic-admission` and `schema-admits-incompatible-binding-requiring-semantic-admission` intentionally pass message-local validation. Semantic admission must reject the disconnected cycle and the incompatible binding. Their names record this requirement; they are not executable plans.

Runtime adapters must also prove graph reachability and acyclicity, port and scope compatibility, authority and capability admission, source receipt identity, legal state transitions, and external cleanup evidence. Replay, dispatch idempotency, authorization, projection behavior, and reconciliation across event streams require separate implementation tests. The runner checks fixture field names before conversion; it does not assert byte-for-byte preservation of an admitted plan or a canonical re-encoding of its wire payload.

See the [workflow architecture proposal](../../../docs/architecture/workflows.md) for the admission, replay, and implementation obligations.

# Sandbox contract

`trogon-sandbox-contract` provides validated worker, dispatch-attempt, and operation
identifiers, operation kinds, and the operation state transition rules.

`WorkerId`, `DispatchAttemptId`, and `OperationId` serialize as JSON strings. Each
accepts 1 to 128 bytes containing only ASCII letters, digits, `_`, or `-`.
Construction, parsing, and deserialization enforce the same grammar and return
typed validation errors.

`OperationKind` uses the JSON strings `create`, `restore`, `fork`, `snapshot`,
`pause`, `resume`, `suspend`, `wake`, `stop`, `delete`, and `rollback`. A kind's
presence in the contract does not establish runtime support for that operation.

`OperationState` uses the JSON strings below. Transitions are allowed only to the
listed destinations:

| State | Allowed destinations |
| --- | --- |
| `queued` | `running`, `canceled`, `expired` |
| `running` | `running`, `queued`, `succeeded`, `failed`, `canceled` |
| `succeeded` | None |
| `failed` | None |
| `canceled` | None |
| `expired` | None |

The `running` self-transition permits takeover of a stale claim, while returning
to `queued` permits retry after a transient failure. Terminal states are immutable,
including self-transitions.

This package defines values and allowed transitions. It does not execute
operations, persist state, authorize transitions, enforce claim ownership, or
provide worker, provider, transport, or lease-generation behavior.

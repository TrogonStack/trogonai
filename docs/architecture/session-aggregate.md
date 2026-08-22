# Session Aggregate

The [Session](../glossary/session) aggregate is the write side of the Session
Store: one event-sourced stream per session, one fold that turns that stream
into decision state, and one command per transition. This page documents the
wire contract that exists today, which is protobuf only. The Rust
[decider](../glossary/decider) is not implemented yet, and the last section says
exactly what it is waiting on.

[ADR#0035](../adr/0035-session-store-decider-aggregate.md) is the decision this
implements; [ADR#0045](../adr/0045-event-sourced-service-module-layout.md) is the
layout it follows. Read [Decider](./decider.md) first for the
`decide`/`evolve`/`initial_state` cycle these types plug into.

## Shape of the contract

Three kinds of protobuf message live under
`proto/trogonai/session/sessions/`, and confusing them is the most common way to
get this wrong:

| Kind | Location | Role |
| --- | --- | --- |
| Events | `v1alpha1/*.proto`, past participle (`SessionStarted`, `ToolCallCompleted`) | recorded facts, the only durable truth |
| Commands | `v1alpha1/*.proto`, present tense (`CreateSession`, `StartToolCall`) | untrusted requests, validated then discarded |
| State | `state/v1alpha1/state.proto` | the fold's output, and the aggregate snapshot payload |

Commands are separate wire messages from events on purpose. An event is a fact
the system has committed to; a command is a request that may be rejected,
duplicated, or malformed. Sharing one message between the two would make the
type system stop distinguishing them.

`State` doubles as the aggregate snapshot payload, tagged by its own
`buffa::MessageName::FULL_NAME`, following the scheduler precedent. No separate
snapshot proto exists or is needed.

## What state holds, and what it refuses to hold

`State` carries exactly the facts some command reads to enforce an invariant.
That is the whole selection rule, and it is why the message is much smaller than
a session transcript:

- session lifecycle: `state` (missing, active, terminal), `terminal_marker`,
  `session_id`, `position`, `execution_plan_digest`
- history shape: `fork_origin`, `keep_through` (newest rewind boundary),
  `compaction` (newest marker)
- lineage: `parent`, `delegations`
- attempts: `active_attempt`, `last_attempt_id`, `last_attempt_number`
- reconciliation surfaces: `tool_calls`, `operations`, `checkpoints`
- privacy dependencies: `redacted_event_ids`, `erased_artifact_ids`

Deliberately absent: message bodies, todo items, artifact lists, display name,
archived flag, assistant-message entries. Every one of those is real session
data, and none of it is read by any command, so all of it belongs to a
projection. Putting it in `State` would mean every replay pays to rebuild a
transcript in order to answer a yes-or-no question.

`position` is a `SessionOrdinal`: the session's own 1-indexed, fold-derived
position, never a JetStream sequence. Domain references point at ordinals so
that history references survive anything the transport does to sequence numbers.

Rewind is the clearest case of the state proto encoding a policy. `keep_through`
masks effective history for readers and future turns, but it never un-applies a
folded fact. A tool call that really ran still has to be reconcilable after a
rewind, so its ledger entry stays.

## Preconditions

Each command declares which concurrency guard its append runs under. All 42 rows
of ADR#0035's matrix fall into three classes:

| Class | Count | Meaning |
| --- | --- | --- |
| `NoStream` | 4 | creation batches: `CreateSession`, `ForkSession`, `CreateChildSession`, `RecoverSession`. A redelivered create observes the stream already exists rather than forking a second history. |
| `At` | 22 | invariant-bearing transitions. The append is guarded against the exact position the decision was made from. |
| `Any` | 16 | commuting happened-facts. No invariant a concurrent writer could break, so no guard. |

`RecoverSession` is the fourth creation batch and the only one whose precondition
is load-bearing for an operator workflow rather than for a user action: a
salvage derives its target session id before writing, so a retried salvage lands
on the same stream and `NoStream` turns the duplicate into a visible conflict
instead of a second copy. See [Session Maintenance](./session-maintenance.md).

`StreamExists` is never used.

`At` needs no trait-level support, and asking for it would be a mistake.
`CommandExecution` already falls back to `At(current_position)` whenever the
stream is non-empty and neither the decider nor the caller declared a
precondition, so a command gets real optimistic concurrency by leaving
`WRITE_PRECONDITION` unset. ADR#0035 rejects a trait-level `At(N)` variant
precisely because a compile-time `const` could only weaken that default, never
strengthen it.

`Any` is a claim about commutativity, and it is only honest because the fold
rules below make ordering irrelevant for those events.

## Fold rules that make `Any` safe

Sixteen commands append without a guard, which means two writers can interleave.
Order-independence is bought by five rules in `evolve`:

- **First terminal outcome wins, per entity.** `ToolCallCompleted` and
  `ToolCallFailed` compete on `tool_execution_id`; `AssistantMessageCompleted`
  and `AssistantMessageFailed` compete on `message_id`. The first in fold order
  is authoritative. A later conflicting outcome stays on the log and is surfaced
  by a projection flag, never folded.
- **Orphans fold as unjoined.** A start with no matching request, an outcome
  with no matching start: retained, flagged, and changing no entity state. They
  are not refused, because refusing them would discard a fact that happened.
- **Highest revision wins.** `TodoUpdated` carries a monotonic `revision` from a
  single logical writer; the fold keeps the highest, ties resolving to the first
  in fold order. Keeping the highest rather than the latest is what makes it
  order-independent.
- **First checkpoint evidence wins,** per `checkpoint_id`. A later event reusing
  the id never replaces admitted evidence, so a restore always resolves to the
  same bytes.
- **Post-terminal facts are retained but audit-only,** with one carve-out:
  guarded ledger and lineage facts still fold after a terminal marker, so an
  in-flight side effect stays reconcilable.

That carve-out is load-bearing. Without it, cancelling a session would strand
every operation still in flight, because the fact that settles them could no
longer be recorded.

## The operation ledger

External side effects are guarded by a ledger, not by hope:

```
ReserveOperation  ->  OperationReserved
                        {operation_id, request_digest, operation_kind}

RecordOperationOutcome -> OperationOutcomeRecorded
                        oneof { succeeded | failed | cancelled | unknown }

RequestOperationCancellation -> OperationCancellationRequested
```

Reservation happens before the side effect runs and records a digest of the
request it is reserved for, so a retry arriving with different bytes under the
same id is refused rather than executed as if it were the same call.

`unknown` is the status that makes crash recovery honest. It means the side
effect may or may not have run, it is explicitly non-terminal, and it may be
superseded exactly once by a determinate outcome. Collapsing it into "failed"
would assert something nobody knows.

Cancellation is a request, not a settlement. `cancellation_requested` is a
separate flag from `status`; the operation still needs a recorded outcome before
it leaves the ledger.

## Worked example: a runner that dies mid-tool-call

This is the case the whole design is shaped around. A runner appends
`ToolCallStarted`, begins the side effect, and the process dies before any
terminal outcome is recorded.

Replaying the session yields:

```
ToolCall {
  tool_call_id:      "...",
  tool_execution_id: "...",
  status:            TOOL_CALL_STATUS_STARTED,
  started_at:        <ordinal>,   // set
  settled_at:        <unset>,     // nothing settled it
}

Operation {
  operation_id: "...",
  status:       OPERATION_STATUS_RESERVED,
  settled_at:   <unset>,
}
```

Both facts are readable directly off the fold. A reconciler does not have to
infer the gap from adjacency in fold order, which concurrent `Any` appends make
unsound anyway. It sees a started call with no outcome and a reserved operation
with no outcome, and it can then:

- finish the call, if the side effect can be confirmed to have completed:
  `CompleteToolCall` plus `RecordOperationOutcome{succeeded}`
- reject it, if the side effect can be confirmed not to have run:
  `FailToolCall` plus `RecordOperationOutcome{failed}`
- record honest ignorance, if neither can be confirmed:
  `RecordOperationOutcome{unknown}`, superseded later when the truth is learned

`RecordOperationOutcome` remains admissible after the session reaches a terminal
state, so a call stranded by a cancellation is still reconcilable rather than
orphaned.

The three distinctions doing the work here are: `started_at` set with
`settled_at` unset means interrupted, not failed; `RESERVED` means the side
effect's fate is unknown, not that it did not happen; and `UNKNOWN` is a status
in its own right rather than a synonym for failure.

## Identity and deduplication

Commands arrive at least once. Every command carries a caller-supplied
idempotency key, and event ids are derived deterministically as UUIDv5 over the
resolved stream subject, the command type, the command idempotency key, and the
index within the batch. `Nats-Msg-Id` carries the event id, so a redelivery
publishes byte-identical ids and collapses in the duplicate window.

The command protos carry no idempotency-key field. Where that key structurally
lives is an explicitly open question in ADR#0035, and inventing a field here
would prejudge a decision that has not been made.

What a caller is told when an append is interrupted, and what a retry is then
allowed to do, is the other half of this and lives in
[Session Crash Boundaries](./session-crash-boundaries.md).

## Effects, faults, and work that outlives its turn

Four parts of the catalog are argued on their own pages rather than here.

What a tool call did beyond returning a result, which covers namespace access as
distinct from content access, copy provenance, and partial outcomes across
multiple targets, is [Session Tool Effects](./session-tool-effects.md).

A tool call that returns a handle while its work keeps running past the turn is
[Session Detached Work](./session-detached-work.md).

A provider emission that was shaped like a tool call and could not become one is
[Session Provider Faults](./session-provider-faults.md).

Three things proposed for this catalog and deliberately kept out of it, with the
condition that would reverse each, are in
[Session Schema Boundaries](./session-schema-boundaries.md).

## Status and what it is waiting on

Shipped: `state/v1alpha1/state.proto` and 42 command protos, lint-clean,
formatted, building, and generating Rust bindings.

Not shipped: `initial_state`, `evolve`, `decide`, the command handlers,
projections, snapshots, the query service, and the recovery operator.

The Rust runtime is blocked on three shared-crate obligations that ADR#0035
raises as requests to the decider platform, not decisions it can make alone:

1. **Evolve-visible event identity**, on both the native and WASM replay paths.
   `ApplyRedaction` guards that its targeted event ids exist in-session, and that
   is uncheckable from folded state while `evolve` cannot see an event's id.
   This would change the near-frozen core `Decider` fold surface for every
   business, so whether it belongs there at all is undesigned.
2. **A deterministic id-derivation hook.** The UUIDv5 formula needs the resolved
   subject, command type, idempotency key, and batch index. The runtime's
   existing `EventIdentity::event_id` override point sees only the event, so it
   cannot host this.
3. **Duplicate-publish-ack treated as success** rather than an append error.
   This first requires the canonical command digest to be recoverable at the ack,
   otherwise the append path cannot tell an idempotent retry from key reuse with
   different content.

Separately, shared multi-tenant deployment is blocked on
[ADR#0027](../adr/0027-decider-multi-tenancy-primitive.md)'s resolver contract.
No `tenant_id` is added speculatively ahead of it; until it lands, a session is
scoped by its subject alone.

The `v1alpha1` suffix is an honest signal rather than a placeholder. It becomes
`v1` when those obligations are met and the draft ADRs this contract depends on
are accepted.

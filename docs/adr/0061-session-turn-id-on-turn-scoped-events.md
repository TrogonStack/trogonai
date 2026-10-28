---
number: "0061"
slug: session-turn-id-on-turn-scoped-events
status: draft
date: 2026-09-11
---

# ADR#0061: Turn Identity Belongs on Every Turn-Scoped Session Event, Not Only the Conversation and Tool Ones

## Context

[ADR#0035](./0035-session-store-decider-aggregate.md) facet 3 settles that turn
identity is stamped and never inferred. The reasoning is not in question and
this ADR does not reopen it: a turn is one user-prompt-to-final-assistant-message
cycle, it is "the unit a person names when they ask to rewind, retry, or cost out
work", and it cannot be folded because concurrent `Any`-precondition appends give
no reliable "next event after" relation to infer membership from.

The same facet then enumerates where the id goes. It is required on
`UserMessageRecorded`, the three `AssistantMessage*` events, and
`ToolCallRequested`/`Started`/`Completed`/`Failed`, optional on
`ToolCallApproved`/`Denied`, and required on `FileChanged`. The enumeration
follows from a scope sentence one paragraph earlier: the id is carried "on every
conversation and tool event".

That scope is drawn around the wrong set. `SessionEvent` has 43 arms; 12 carry
`turn_id` and 31 do not. Most of the 31 are correctly excluded, because they are
session-lifetime or cross-stream facts that no turn owns. But eight of them
record work that only ever happens because a turn is executing, and they are
precisely the events the three stated use cases need:

- **Cost out work.** The operation ledger is where side effects are reserved and
  settled. A turn's cost is its model usage plus its side effects, and the side
  effects are unattributable today.
- **Retry.** `OperationReserved` and `OperationOutcomeRecorded` are what make a
  retry safe, and "retry this turn" cannot enumerate the operations the turn
  reserved.
- **Rewind.** `DelegationDispatched` names a child session spawned by a turn.
  Rewinding past that turn has to know the delegation belongs to it.

The gap is also internally inconsistent independent of any of that.
`ToolCallCompleted` carries `turn_id` even though it joins to
`ToolCallRequested` by `tool_call_id` and could recover the turn from there.
Facet 3's own argument is that a join reachable only by walking fold order is
not a substitute for a stamped field. `OperationReserved` joins to the tool call
by `operation_id` through exactly the same kind of walk, and gets no stamp.
Either the join is good enough in both places or in neither.

Finally, the window that allows fixing this at all is closing. The `events.proto`
preamble states that within `v1alpha1` a field may still be added as
`LEGACY_REQUIRED`, but "that window is open only while both conditions hold --
no deployed producer has written these events, and this package has not promoted
-- and it closes at whichever comes first". No Rust producer constructs these
events yet, so the first condition still holds. `buf breaking` under `WIRE_JSON`
does not model field presence and will not report a missed one, so the deadline
is not enforced by CI.

The occasion for looking was the [OpenAI Agents API
dossier](../research/agent-platform/products/openai-agents-api.md), whose
contract impact is recorded in
[contract impact: OpenAI Agents API](../research/agent-platform/contract-impact-agents-api.md)
as delta D2. That product ships a first-class `turn` resource carrying its own
`usage`, which is what prompted the audit. The defect it surfaced is ours and
stands on its own evidence; nothing in this ADR depends on matching that vendor.

## Decision

### 1. The scope rule is "produced while executing a turn", not "conversation and tool event"

An event carries `turn_id` when a turn's execution is the only thing that
produces it. The test is not what the event is about, it is what had to be
running for it to exist. A tool call and an assistant message pass that test, and
so does every reservation in the operation ledger, every delegation dispatch,
and every plan update, none of which a session produces while idle.

The events that continue to carry no turn id are the ones that fail the test, and
they fail it for two distinct reasons worth keeping separate:

- **Session-lifetime facts** happen to the session rather than inside a turn:
  `SessionStarted`, `SessionRenamed`, the terminal family, the archive and hide
  family, `SessionForked`, `SessionRewound`, `RedactionApplied`,
  `ArtifactErased`.
- **Range-scoped and cross-stream facts** already address turns by another
  means, and stamping a single id on them would be false. `Compacted` and
  `CheckpointProduced` carry `SessionOrdinal` ranges (`covers_from`,
  `covers_through`) that span many turns by construction. The `Parent*` family
  and `DelegationDetached` are facts about another stream's turn, not this
  one's. `ExecutionAttempt*` events bound an attempt, which contains turns
  rather than belonging to one.

### 2. The eight events, and which two are optional

| Event | Field | Presence |
| --- | --- | --- |
| `OperationReserved` | `turn_id = 5` | required |
| `OperationOutcomeRecorded` | `turn_id = 7` | required |
| `OperationCancellationRequested` | `turn_id = 4` | required |
| `DelegationDispatched` | `turn_id = 6` | required |
| `ExternalDelegationDispatched` | `turn_id = 8` | required |
| `TodoUpdated` | `turn_id = 4` | required |
| `ArtifactRecorded` | `turn_id = 3` | optional |
| `SystemNoticeRecorded` | `turn_id = 5` | optional |

The two optional fields follow the precedent facet 3 already set for
`ToolCallApproved`/`Denied`, where "an external approver may hold the call
identity without the turn context". The same condition holds here and it is a
statement about the producer, not about the event's importance.

`ArtifactRecorded` is a claim-check registration whose producer is not always
inside a turn. A reconciler repairing a crash, an out-of-band upload, and fork
inheritance all register artifacts with no turn in hand. Where an artifact does
belong to a turn it is reachable anyway, because the event that references it
(`ToolCallCompleted.observed`, `OperationOutcomeRecorded.response_ref`) is a
direct reference from an event that carries the id, not a walk over fold order.

`SystemNoticeRecorded` already documents itself as a notice "that is not tied to
a specific assistant turn". Making it required would contradict the message's own
stated purpose.

Requiring an id that a legitimate producer cannot supply forces a sentinel value,
and a sentinel is worse than an absent field: it is indistinguishable from a bug
at read time, and unlike an absent field it cannot be tightened later without a
migration.

### 3. The command carries the id and the event repeats it

Each of the eight commands gains the matching field with the matching presence,
as `RequestToolCall` and `ToolCallRequested` already do. Field numbers are kept
equal across the command and event pair wherever the existing numbering allows
it, which is everywhere except `DispatchDelegation` (`turn_id = 5`) and
`DelegationDispatched` (`turn_id = 6`). That pair is already misaligned on
`cascade_policy`, which occupies 4 on the command and 5 on the event, so parity
was not available to preserve. The event's unassigned field 4 is left unassigned
and is not marked `reserved`: it was never used by any version of the message, so
declaring it retired would put a false statement on the record.

### 4. The ledger events take the id from the command, not from folded state

For `RecordOperationOutcome` and `RequestOperationCancellation` the decider
already holds the reservation in state and could copy `turn_id` from it instead
of trusting the command. It does not, for consistency with `CompleteToolCall`,
which carries `turn_id` even though `ToolCallRequested` already recorded it.

The decider validates rather than derives: when the command's `turn_id` does not
match the reservation's, the command is refused as a typed conflict, the same
shape `DelegationDispatched` already uses for a mismatched `cascade_policy` copy
at child creation. This keeps one writer's claim checkable against the log
instead of silently overwritten by it.

### 5. This lands before the first producer or not at all

These are `LEGACY_REQUIRED` additions inside an open window, and the window's
first condition closes the moment any deployed producer writes one of these
events. After that the same change is a decode-breaking migration for every
event already written. CI will not catch the transition, so it is a sequencing
obligation on this ADR, not a check.

If this ADR is rejected, the eight fields are not added later as required ones.
The fallback is optional fields plus a documented statement that turn attribution
for ledger, delegation, and plan facts is best-effort.

## Alternatives Considered

### Leave the gap and reconstruct turn membership at read time

A reader could walk `operation_id` to the tool call that reserved it and take
that call's `turn_id`. This is the reconstruction facet 3 rejected, and it is
weaker here than it is for tool calls: an operation reserved by something other
than a tool call, such as a delegation dispatch, has no such join at all.

### Derive the id in the decider from folded state instead of taking it from the command

Sound for the three ledger events that follow a reservation, and it removes a
mismatch class outright. Rejected because it splits the package into two
conventions for the same field, so a reader could no longer tell from a command
proto whether the id it carries is authoritative. Facet 4 keeps the single
convention and recovers the safety property by validating the copy.

### Stamp a turn id on every event, uniformly

Simple to state and wrong on the range-scoped events. `Compacted` covers an
ordinal range spanning many turns; any single id on it would be a lie, and a lie
that a projection would happily index.

### Add the fields as optional and tighten them after the first producer ships

Tightening presence after events exist is a decode-breaking migration, which is
the exact failure the `events.proto` window rule exists to prevent. Optional is
the right answer for the two producers that genuinely lack the context, and the
wrong answer for the six that have it.

## Consequences

- "What happened in this turn" becomes a filter over a decoded field for the
  ledger, delegation, and plan facts, as it already is for conversation and tool
  facts. Turn-level cost attribution becomes expressible, since a turn's side
  effects can be enumerated without a join.
- Eight commands gain a required or optional field that every caller must
  populate. Six of them are required, so a caller that omits one is refused at
  the storage boundary rather than accepted and left unattributable.
- [ADR#0035](./0035-session-store-decider-aggregate.md) facet 3's enumeration of
  where `turn_id` is required is extended by this ADR. Facet 3 remains the source
  of the rule; this ADR is the source of the list.
- The decider gains a typed conflict for a `turn_id` that disagrees with the
  reservation it settles, on the three ledger commands.
- The `v1alpha1` required-field window is spent on this change for these eight
  events. Any further turn-scoped event added to the package carries the id from
  the start.
- The remaining deltas from the Agents API contract-impact analysis (an
  environment noun, a steering enum, the subagent-versus-child-session question,
  an `OPERATION_KIND_ENVIRONMENT_PROVISION` arm, MCP connection origin) are
  deliberately not decided here and remain open.

## References

- [ADR#0031](./0031-agent-implementation-and-session-plan.md) section 3, where
  the Session's durable operation ledger and its reserve-then-settle contract are
  defined.
- [ADR#0035](./0035-session-store-decider-aggregate.md), facet 2 for the
  commuting-append precondition that makes turn membership unfoldable, and facet
  3 for turn identity being stamped rather than inferred.
- `proto/trogonai/session/sessions/v1alpha1/events.proto`, the preamble stating
  the `LEGACY_REQUIRED` window and its two closing conditions.
- `proto/trogonai/session/sessions/v1alpha1/user_message_recorded.proto`, the
  canonical statement of what a `turn_id` is and why it is stamped.
- [Contract impact: OpenAI Agents API](../research/agent-platform/contract-impact-agents-api.md),
  delta D2, where the gap was first written down.

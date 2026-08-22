# Session Provider Faults

A provider emits a tool call with truncated JSON arguments, or with an empty id,
or with an id it already used. The call cannot run. This page documents where
that fact goes, and why it could not go anywhere that already existed. It
documents the protobuf contract that exists today. There is no Rust
implementation yet.

See [Session Aggregate](./session-aggregate.md) for the tool call lifecycle this
sits outside of.

## The malformed intent has nowhere to live

This is not a design preference. The canonical transcript shape for a tool call
is `ToolUseBlock`, and its `input_json` is required and must be valid JSON. An
intent whose arguments are truncated JSON has nothing to put in that field. The
shape structurally cannot hold the thing that went wrong.

The raw provider payload is retained, in `ProviderBlock`. But `ProviderBlock` is
documented write-verbatim, read-never: no projection may interpret it. Mining it
to answer "why did this turn do less than it looked like it did" would make a
read model depend on a shape the domain explicitly promised never to read, and
the promise is what lets providers change their payloads without breaking
anything downstream.

So the options were a typed event or nothing. Nothing means the most common
provider fault in production leaves a session that silently did less than it
appeared to, with the evidence sitting in a field no reader is allowed to open.

`ProviderToolIntentRejected` is the typed event.

## It is not a denial

`ToolCallDenied` is the closest existing event and it is the wrong one.

A denial is a well-formed request that a human, a policy, or a hook refused. The
request exists, it is on the log as `ToolCallRequested`, and the denial is a
decision about it. Here there is no admissible request at all. Nothing is
written to `ToolCallRequested`, no execution id is minted, and no operation is
reserved.

Refusing to synthesize a request is the point. Manufacturing a `ToolCallRequested`
out of unparseable input would put a call on the permanent log that the provider
never validly asked for, and every later reader would see the session as having
attempted something it did not.

## It is per intent, not per message

A model can emit ten tool calls in one message. If nine are sound and the tenth
is malformed, the nine must still run.

That is why this is not a new `AssistantMessageFailureReason`. Marking the
message failed would discard nine good calls in order to describe one bad one.
The nine proceed to `ToolCallRequested` normally, and only the tenth lands here,
carrying `message_id` so the association is still recoverable.

## The claimed id is a claim, never an identity

`claimed_tool_call_id` holds whatever the provider said, verbatim, including
empty. It must never be joined against `tool_call_id` anywhere.

The whole point of the duplicate case is that the value collides with a real
call. Admitting it into a join would attribute one call's result to another and
make every downstream correlation quietly wrong. The field name says "claimed"
because that is all it is: a record of what was asserted, kept for diagnosis, not
for lookup.

The same reasoning explains why `rejection_id` is minted by the runtime rather
than taken from the provider. The entire fault class is that the provider's
identifier is missing, empty, or duplicated. It cannot be the key.

## The raw payload goes out of line

`raw_intent` is a claim-check, not inline bytes.

Malformed input has no length bound, and this is the one payload in the system
whose size is set by whatever produced the fault. A megabyte of truncated JSON
inlined into a log that is never truncated
([ADR#0035](../adr/0035-session-store-decider-aggregate.md) facet 7) makes one
bad generation permanent for every reader of the stream, forever. It goes to an
artifact, like every other unbounded payload, and it is erasable like one.

Unset means the raw emission could not be captured, which is worth
distinguishing from a capture of nothing.

## Why the reasons are typed

`ProviderToolIntentRejectionReason` is an enum rather than a string because the
operational responses genuinely differ:

| Reason | What it means operationally |
| --- | --- |
| `MALFORMED_ARGUMENTS` | a prompt or model problem, often a token limit |
| `SCHEMA_VIOLATION` | the tool's declared schema and the model's understanding disagree |
| `MISSING_CALL_ID` | a provider protocol violation |
| `DUPLICATE_CALL_ID` | a provider bug that corrupts joins if ever admitted |
| `UNKNOWN_TOOL` | catalog drift between what was offered and what is registered |
| `UNRESOLVABLE_PARENT` | the call tree cannot be reconstructed |
| `OVERSIZED` | refused before parsing, as an input bound |

Collapsed into one free-text reason, all seven become a single unactionable
alert.

## Layout

`proto/trogonai/session/sessions/v1alpha1/provider_tool_intent_rejected.proto`
and its paired command
`proto/trogonai/session/sessions/v1alpha1/reject_provider_tool_intent.proto`.

| Type | What it settles |
| --- | --- |
| `ProviderToolIntentRejected` | the intent existed, was refused, and never became a call |
| `ProviderToolIntentRejectionReason` | which fault, in terms an operator can act on |
| `RejectProviderToolIntent` | the command, with the payload already stored |

It is arm 43 of `SessionEvent`, and a commuting happened-fact
(`WRITE_PRECONDITION = Any`,
[ADR#0035](../adr/0035-session-store-decider-aggregate.md) facet 2): the provider
already did it, and no lifecycle state can make it untrue.

## Status

Shipped: both protos, lint-clean, formatted, building, wired into the event
catalog and its codec and validator, with tests.

Not shipped: the provider adapter that detects these faults and issues the
command. Nothing produces the event yet.

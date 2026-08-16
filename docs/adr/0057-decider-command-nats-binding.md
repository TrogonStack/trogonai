---
number: "0057"
slug: decider-command-nats-binding
status: draft
date: 2026-08-15
---

# ADR#0057: Decider Command NATS Binding

## Context

The WASM decider path is complete as a library. A component declares its command
types in a `module-descriptor`, `DeciderRegistry` routes a command type to the
module that claims it, `WasmCommandExecution` runs the load/decide/append/snapshot
cycle, and `JetStreamStore` persists the result. Nothing puts that library behind
a network surface, so today the only way to execute a WASM decider command is to
link the runtime into a process and call it in-process.

The host crate that closes that gap needs a wire contract, and there is none.
[ADR#0016](./0016-protobuf-rpc-over-nats-micro-binding.md) binds annotated
protobuf **services** to NATS micro and says in its own terms that
"specializations (for example decider commands) are defined in their own ADRs".
[ADR#0055](./0055-nats-subject-design-jsonrpc-bindings.md) governs the
message-oriented JSON-RPC and operational subjects and explicitly excludes
protobuf RPC. A decider command falls between the two: it is protobuf, but it is
not an `rpc` method on a `service`.

Three properties of the decider path constrain the answer, and each one rules
out an otherwise obvious choice:

- **The routable set is dynamic.** `DeciderRegistryHandle::activate` and
  `retire` change which command types are routable while the process is running,
  because that is how a module version is rolled out. Any binding whose
  subscriptions are derived from a static descriptor has to re-synchronize its
  subscription set on every rollout.
- **The command type is already a protobuf type URL.** `CommandType` is exactly
  the `type.googleapis.com/...` URL the guest declares, and the registry is keyed
  by it. A subject scheme that names commands some other way needs a mapping
  table that can drift from the registry.
- **Durability already has an owner.** A decider command's durable effect is the
  events it appends to JetStream. Nothing about the request is the source of
  truth.

## Decision

### 1. A decider command host is not a NATS micro service

It is a core NATS request/reply responder with the contract below.

This is a deliberate departure from [ADR#0016](./0016-protobuf-rpc-over-nats-micro-binding.md)
rather than an oversight, and the reason is 0016's own invariant: "The registered
micro service exposes exactly the annotated `service`'s methods as endpoints." A
decider host cannot satisfy it. There is no annotated `service` to enumerate, and
the set it would enumerate instead is whatever modules are activated at this
instant, which changes under `activate` and `retire`. Registering as a micro
service would mean either freezing the routable set at startup (giving up runtime
rollout, the feature the registry handle exists for) or mutating a micro service's
endpoint set at runtime and reporting a discovery record that is a snapshot of a
moving target.

What 0016 provides beyond the substrate is discovery, per-endpoint stats, and a
standard error channel. Discovery of decider commands is answered by the registry
(`DeciderRegistryHandle::routes` reports every routed command type with the
module name and version serving it) and is exposed operationally rather than
through `$SRV`. Stats and errors are covered by the observability rule in section 7
and the outcome taxonomy in section 5.

### 2. Commands are core request/reply, never a JetStream submission

The command subject is a core NATS subject. It is not backed by a stream, and a
host does not acknowledge, redeliver, or dead-letter a command.

Durability belongs to the events, not the request. Persisting the request too
would create a second source of truth for "did this happen" that can disagree
with the event stream, and it would introduce a redelivery loop whose retries the
original caller cannot observe. A caller that needs durable submission puts its
own work queue in front of the host and drives this binding from its consumer;
that keeps the retry policy with the party that knows the business deadline.

### 3. The subject is the command's protobuf type

```text
{prefix}.{proto_full_name}
```

`prefix` is the configured subject namespace, a dotted NATS token defaulting to
`decider`. `proto_full_name` is the command message's fully qualified protobuf
name: the `CommandType` type URL with its `type.googleapis.com/` prefix stripped.

```text
decider.trogonai.scheduler.schedules.v1.CreateSchedule
```

The projection is total and bidirectional by construction, because it is the
identity on the type URL's message name. There is no mapping table to publish and
nothing that can drift from the registry: the host recovers the exact
`CommandType` it routes on from the subject alone, and rejects a subject whose
recovered command type no activation claims.

Following [ADR#0016](./0016-protobuf-rpc-over-nats-micro-binding.md), the subject
carries no `v{major}` binding token and its terminal is not lower_snake. The
command's own proto package already carries its version (`...schedules.v1.`), and
a binding-version token would version the routing contract a second time in the
same subject. The terminal is the protobuf message name verbatim, PascalCase
included, because any casing transformation would break bidirectionality for the
message names that differ only by word boundary.

**A responder subscribes to the whole subtree.** One `{prefix}.>` subscription on
a queue group covers every command type, including types activated after the
subscription was created. This is not an optimization: it is what makes runtime
module activation work without a subscription-management protocol. A responder
with one subscription per command type would have to add and drop subscriptions
in lockstep with every `activate` and `retire`, and a rollout would have a window
in which the routing table and the subscription set disagree.

**The subject names the command, never the module.** Neither the module name nor
the module version appears in it. A module version swap is invisible to callers
by design (see `DeciderRegistryHandle`'s rollout semantics), and putting either in
the subject would publish a deployment detail as part of the caller's contract and
break every caller on rollout.

Subjects are checked against
[ADR#0055](./0055-nats-subject-design-jsonrpc-bindings.md)'s shape and limit rules
with `trogon_nats::validate_published_subject`, which is where the token, byte,
and layout budgets live. The casing rule does not apply, per the paragraph above,
which is why the check is the published-subject pass rather than the pattern pass.

### 4. The request is the encoded command message

The request payload is the command message's protobuf encoding with
`Content-Type: application/protobuf`. The message **type** comes from the subject
and is never parsed from the body, matching
[ADR#0016](./0016-protobuf-rpc-over-nats-micro-binding.md)'s invariant. The host
constructs the WIT `command-envelope` by pairing the subject-derived type with the
request bytes; it does not decode the payload, because only the guest knows the
schema.

Two headers carry what the payload cannot:

| Header | Meaning |
| --- | --- |
| `Trogon-Command-Id` | UUID identifying **this command**, not this delivery attempt. Makes retries idempotent (see section 6). Optional. |
| `Trogon-Expected-Revision` | The stream revision the caller believes it is acting on, as a decimal `u64`. Strengthens the module's declared write precondition; can never weaken it. Optional. |

Both are optional and both are rejected with `InvalidRequest` when present but
unparseable, rather than silently ignored: a caller that sends an idempotency key
the host drops would believe it has a guarantee it does not have.

Trace context propagation follows
[ADR#0042](./0042-nats-trace-context-and-message-path-tracing.md).

### 5. Every reply is one `CommandOutcome`

The reply payload is always a `trogonai.decider.v1.CommandOutcome`, whose `oneof`
has exactly five arms: `decided`, `rejected`, `faulted`, `shed`, `denied`. There
is no separate success body and error body, and there is no reply-is-an-error
header convention. A caller decodes once and matches, and no outcome can be added
later without every caller's match seeing it.

The `Trogon-Decider-Outcome` reply header carries the same discriminant so
middleware can route and meter without decoding the body. **Its value space is
the `decision_outcome` telemetry attribute's** (`decided`, `rejected`, `faulted`,
`shed`, `denied`), so the wire vocabulary and the trace vocabulary are one
vocabulary and cannot drift. On disagreement between header and body the body is authoritative,
the inverse of 0016's rule, because here the body is the typed contract and the
header is the derived summary.

**Every arm but `decided` is a `google.rpc.Status`**, per
[ADR#0016](./0016-protobuf-rpc-over-nats-micro-binding.md) section 3: a canonical
`google.rpc.Code`, a human-readable message, and standard `google.rpc` detail
messages in `details`. What this binding adds is the arm, not the shape. 0016
fixes what an error body looks like platform-wide and keeps defined business
outcomes off the micro error channel so `num_errors` stays a health signal; the
`oneof` is how they stay off it here. A rejection is still a `Status`, and it is
still not a service error.

Every status carries a `google.rpc.ErrorInfo`. Its `reason` is the stable,
machine-readable identifier a caller branches on, and its `domain` says who owns
that reason's namespace: `trogonai.decider.v1` for anything the host decided, and
the **module's own name** for a rejection, whose code space belongs to the module
rather than to the host. Without that split, two modules choosing the same
rejection code would be indistinguishable on the wire.

`details` is a `repeated google.protobuf.Any` with no ordering guarantee. A
reader locates a detail by unpacking on its type URL, never by indexing.

`decided` carries the appended events themselves, as `google.protobuf.Any`
payloads in append order, each with the event id it was appended under. A caller
that must act on what it just decided, such as one warming a cache from its own
write, would otherwise have to wait for its own events to come back around off
the event stream, and the host already holds everything that caller needs at the
moment it replies. Withholding the payloads to keep the reply small buys nothing:
the events are bounded by the decision that produced them, and the caller reads
them either way.

The id is what makes the reply and the event stream safe to consume together. It
is the same value the host publishes as the event's `Nats-Msg-Id`, so a caller
that applies this reply and also tails the stream sees one event twice under one
identity and applies it once. A stream position could not serve here: the
`stream_position` this arm carries is the high watermark of the append as a
whole, not a position per event, and per `AppendStreamResponse`'s own contract it
is neither a next-expected-version nor safe to do arithmetic on. It is carried so
a caller can read its own write, and nothing more.

`Any` rather than a type-and-bytes pair, because that pair is what `Any` already
is. The one difference from the form the events carry on the stream is the
`type.googleapis.com/` prefix that the `Any` encoding requires and the
`Trogon-Event-Type` header does not; the host applies it at the reply boundary
and nowhere else.

`rejected` is a reply arm, not an error channel. A module refusing an invalid
command is the decider pattern working: the host executed correctly and the domain
answered no. Counting that as a service error would turn a health signal into a
business-outcome counter, which is the failure
[ADR#0016](./0016-protobuf-rpc-over-nats-micro-binding.md) section 3 warns about.
Its code is `FAILED_PRECONDITION` rather than `INVALID_ARGUMENT`, which is the
distinction `google.rpc.Code` already draws: the command is well-formed and would
succeed against a different stream state.

`shed` is its own arm rather than a fault for the same reason in the opposite
direction: per
[ADR#0028](./0028-decider-admission-control-and-backpressure.md) a shed command
was refused before anything was read, decided, or appended. The host is full, not
broken, and the caller's correct response is backoff rather than escalation. It
is `RESOURCE_EXHAUSTED` and carries a `google.rpc.QuotaFailure` naming the limit
it contended for as a number, so the backoff can be sized rather than guessed.
The violation's `subject` is empty: the limit is the host's, so nothing about who
asked determined that the answer was no.

`denied` is likewise its own arm rather than a fault. Per
[ADR#0026](./0026-command-authorization-principal.md) a denied command was
refused before anything was read, decided, or appended, because the submitting
principal was absent or the host's authorizer refused it. Nothing is broken and
no retry of the same command by the same principal will fare differently, so the
caller's correct response is different credentials rather than backoff, and an
operator counting it is counting attempted access rather than service health. The
two refusals are separate codes because the caller's next move differs:
`UNAUTHENTICATED` says present credentials, `PERMISSION_DENIED` says present
different ones. The message is only what the authorizer gave, written for a caller
to read and not to branch on: a denial that named the rule that refused it would
be telling an unauthorized caller how to become an authorized one.

`faulted` is a single `Status` whose class is read off `ErrorInfo.reason`. A
reason rather than a nested `oneof`, because a code space is what a caller
branches on and `google.rpc` already publishes the convention for one; a
per-class message would also be a second discriminant alongside the code, free to
disagree with it. Two classes share `INTERNAL` on purpose: to a caller they are
one thing, and the reason is what tells an operator which party is answerable.

The mapping from the runtime's error taxonomy is total:

| `WasmCommandError` | Arm | `google.rpc.Code` | `ErrorInfo.reason` | Retry |
| --- | --- | --- | --- | --- |
| `Rejected` | `rejected` | `FAILED_PRECONDITION` | the module's own code | No; the command is wrong, not the moment |
| `Overloaded` | `shed` | `RESOURCE_EXHAUSTED` | `ADMISSION_LIMIT_REACHED` | Yes, after backoff |
| `Unauthorized::MissingPrincipal` | `denied` | `UNAUTHENTICATED` | `PRINCIPAL_MISSING` | No; not without credentials |
| `Unauthorized::Denied` | `denied` | `PERMISSION_DENIED` | `PRINCIPAL_UNAUTHORIZED` | No; not by this principal |
| unrouted command type | `faulted` | `UNIMPLEMENTED` | `COMMAND_TYPE_UNROUTABLE` | Only after a module claiming it activates |
| undecodable subject, unparseable header | `faulted` | `INVALID_ARGUMENT` | `COMMAND_REQUEST_MALFORMED` | No |
| `PreconditionConflict`, optimistic concurrency conflict | `faulted` | `ABORTED` | `STREAM_WRITE_CONFLICT` | Yes; a retry replays the stream as it now stands |
| `Faulted`, `Trap`, `EmptyDecision`, `Evolve`, `StreamId`, `Instantiate` | `faulted` | `INTERNAL` | `GUEST_FAULT` | No; a retry repeats the same guest call |
| `DeadlineExceeded` | `faulted` | `DEADLINE_EXCEEDED` | `GUEST_DEADLINE_EXCEEDED` | Only once the load that slowed the guest has passed |
| `ReadSnapshot`, `ReadStream`, `Append` | `faulted` | `UNAVAILABLE` | `STORAGE_UNAVAILABLE` | Yes, once storage recovers |
| `ReplayLimitExceeded`, `SnapshotAheadOfStream`, `ReadAfterOverflow`, `Blocking` | `faulted` | `INTERNAL` | `HOST_INTERNAL` | No |

Where an error has a cause chain, it travels as `google.rpc.DebugInfo`, whose
`stack_entries` is ordered outermost cause first. A guest's chain crossed the WIT
boundary already flattened into pairs and keeps the guest's own labels, because
the host cannot recover the chain they came from to re-derive them. `DebugInfo`
is omitted rather than sent empty: an empty detail says what an absent one says
and costs a caller a decode to find out.

A command that produced no reply at all (the caller timed out) is not an outcome:
its events may or may not have been appended, which is exactly what section 6
exists to make safe.

### 6. Idempotency is the caller's key, carried end to end

`Trogon-Command-Id` identifies the command, so the same value must be reused
across retries of one logical command and must differ between distinct commands.
The host turns it into a `CommandId`, from which the execution derives each
event's id deterministically, which becomes the event's `Nats-Msg-Id` and is
deduplicated by the events stream. A retry of a command that already appended its
events therefore appends nothing the second time.

Absent the header, event ids are freshly generated and a retry appends a second
copy of the events. That is the honest default: the host cannot invent an
identity for a command whose caller did not give it one, and silently making
retries "probably fine" would hide the one case where they are not.

The window in which this holds is the events stream's duplicate window, not
forever. A retry arriving after that window expires appends again. Deployments
size the window against their longest plausible retry, as the scheduler's already
does.

### 7. Observability reuses the decider's existing attributes

A host records the spans and attributes the runtime already defines
(`command_type`, `decision_outcome`, `guest_phase`, the replay and snapshot
metrics), rather than introducing a transport-specific parallel vocabulary. The
subject is derived from `command_type`, so recording both would be recording the
same fact twice.

## Invariants

- A responder covers its command surface with one `{prefix}.>` subscription;
  per-command-type subscriptions are never used, because they cannot track
  runtime activation.
- The command type is recovered from the subject and never from the request body.
  The host does not decode a command payload.
- Neither the module name nor the module version appears in a command subject.
- Every reply body is exactly one `CommandOutcome`; there is no alternative
  success or error body, and an outcome is never signalled by an absent or
  malformed reply.
- `Trogon-Decider-Outcome` and the `decision_outcome` attribute draw from one
  value space. Adding an outcome means adding it to both, in the semconv registry
  and in `CommandOutcome`.
- A domain rejection is `rejected`, never a fault. A shed is `shed`, never a
  fault.
- Every arm but `decided` is a `google.rpc.Status` carrying a canonical
  `google.rpc.Code` and a `google.rpc.ErrorInfo`. The host defines no error
  message of its own and no `Status` in a non-`decided` arm reports `OK`.
- `ErrorInfo.domain` is `trogonai.decider.v1` for everything the host decided and
  the module's name for a rejection. A host never reports a module's rejection
  code under its own domain.
- A caller reads `Status.details` by unpacking on a type URL. Nothing about the
  order or the count of that list is promised.
- A present-but-unparseable `Trogon-Command-Id` or `Trogon-Expected-Revision`
  fails the command; neither is ever ignored.
- Commands are core NATS. A host never acknowledges, redelivers, or dead-letters
  a command.

## Alternatives Considered

### Register as a NATS micro service per [ADR#0016](./0016-protobuf-rpc-over-nats-micro-binding.md)

Rejected on 0016's own endpoint invariant, as argued in section 1: the routable
command set is not derivable from a static service descriptor because runtime
activation changes it. Adopting micro would have cost either runtime rollout or
the truthfulness of the discovery record. The parts of 0016 that do fit (the
subject is derived mechanically, the type is a property of the endpoint and not
parsed from the body, application-level negative outcomes stay out of the error
channel) are adopted directly in sections 3, 4, and 5.

### Put the command type in the request body instead of the subject

Rejected. One subject for all commands makes subject-prefix authorization
all-or-nothing: an account granted the right to issue any command is granted the
right to issue every command. Deriving the subject from the proto package instead
means `decider.trogonai.scheduler.schedules.v1.>` is a grantable capability that
means exactly "the scheduler's commands". It also puts routing behind a decode,
so a malformed body becomes unroutable rather than merely undecidable.

### Lower_snake the subject terminal to satisfy [ADR#0055](./0055-nats-subject-design-jsonrpc-bindings.md)'s casing rule

Rejected. 0055 does not govern this binding, and the transformation is not
injective: `CreateSchedule` and `Createschedule` both project to
`create_schedule`, so the mapping stops being bidirectional exactly where 0055's
own method-to-terminal rule requires bidirectionality. The subject stays the
protobuf name verbatim.

### Carry commands on a JetStream work queue with durable consumers

Rejected per section 2: it duplicates the durability the event stream already
provides and hides retries from the caller. A deployment that wants it composes
it in front of this binding, which keeps the retry policy where the deadline is
known.

### Model the outcome as an error channel plus a success body

Rejected. It splits one total outcome across two encodings and two decode paths,
and it forces every intermediate to agree on which signal wins. The `oneof` makes
the outcome set exhaustive at the type level; the header keeps the
decode-free metering that the split encoding was there to provide.

### Define per-outcome error messages instead of `google.rpc.Status`

Rejected, and this is where the binding follows
[ADR#0016](./0016-protobuf-rpc-over-nats-micro-binding.md) rather than departing
from it. Section 1 departs from 0016 on the registration model because a decider
host cannot satisfy 0016's endpoint invariant. Nothing about that argument
reaches the error body, and the two questions are independent: 0016 fixes the
shape of an error, and this binding fixes which arm it arrives in.

A hand-rolled `CommandFaulted` / `CommandRejected` / `CommandShed` /
`CommandDenied` family is what a first pass produced, and it cost three things.
It re-invented the canonical code space, so a caller with a generic protobuf
error handler had to special-case the decider. It re-invented the detail types,
including a `DomainErrorDetail` key/value pair whose keys were only an index into
an ordered chain, which is what `google.rpc.DebugInfo.stack_entries` already is.
And it lost information: a single `CommandDenied.reason` string flattened two
refusals whose correct caller responses differ, which the `UNAUTHENTICATED` /
`PERMISSION_DENIED` split now keeps apart.

The residual cost is that `google/rpc/status.proto`, `code.proto`, and
`error_details.proto` enter this schema's transitive closure. That is the price
of a caller being able to read a reply with types it already has.

## Consequences

- A caller needs the `trogonai.decider.v1` messages to talk to a decider host.
  That is one small package, versioned independently of any domain's commands,
  and its only additional dependency is `google.rpc`, which a caller that speaks
  any other Google-API-shaped service already has.
- A generic protobuf error handler works on a decider reply without knowing what
  a decider is. The arm is the only decider-specific thing a caller has to learn.
- Adding a fault class is now a new `ErrorInfo.reason` under an existing code,
  not a new message and not a `oneof` arm. Callers that branch on the code keep
  working; only the ones that branch on the reason see it.
- Callers that want at-least-once submission must build it. This binding gives
  them the idempotency key that makes it safe; it does not give them the queue.
- `Trogon-Decider-Outcome` and the semconv `decision_outcome` member list are now
  coupled. A change to the outcome set is a change to the semconv registry, the
  generated attribute enum, and `command_outcome.proto` together.
- Because subjects are derived from proto packages, an authorization policy
  written against them stays correct as modules are added, and only changes when
  a new proto package appears.

## Related ADRs

- [ADR#0009: Protocol Buffers Wire Contracts](./0009-protocol-buffers-wire-contracts.md)
- [ADR#0016: Protocol Buffers RPC over NATS micro Binding](./0016-protobuf-rpc-over-nats-micro-binding.md)
- [ADR#0028: Decider Admission Control and Backpressure](./0028-decider-admission-control-and-backpressure.md)
- [ADR#0042: NATS Trace Context and Message Path Tracing](./0042-nats-trace-context-and-message-path-tracing.md)
- [ADR#0045: Aggregate-Oriented Module Layout for Event-Sourced Services](./0045-event-sourced-service-module-layout.md)
- [ADR#0055: NATS Subject Design for JSON-RPC Protocol Bindings](./0055-nats-subject-design-jsonrpc-bindings.md)

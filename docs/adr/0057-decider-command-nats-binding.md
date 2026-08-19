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
  by it. Any scheme that names commands some other way needs a mapping table that
  can drift from the registry.
- **Durability already has an owner.** A decider command's durable effect is the
  events it appends to JetStream. Nothing about the request is the source of
  truth.

## Decision

### 1. A decider command host serves an annotated protobuf service

The binding follows [ADR#0016](./0016-protobuf-rpc-over-nats-micro-binding.md)
rather than departing from it. There is an annotated `service`, it is the
canonical wire contract, and its method set is fixed.

The apparent obstacle was 0016's endpoint invariant: "the registered micro
service must expose exactly its methods as endpoints." A decider cannot enumerate
its command types as methods, because which types are routable changes under
`activate` and `retire` while the process runs, and freezing that set into a
descriptor would cost the runtime rollout the registry handle exists for.

The resolution is that the open set does not have to live in the method set. One
method, `Decide`, takes the command as a `google.protobuf.Any` in its request.
The descriptor is static, the endpoint set is static, and what varies at run time
varies inside a field. 0016's invariant holds exactly, and the caller gets what a
generated client gives it: the subject, the request and response types, and the
error convention all read off one descriptor.

Discovery of *which commands* are routable is still the registry's answer, not
the descriptor's. `DeciderRegistryHandle::routes` reports every routed command
type with the module name and version serving it, exposed operationally. The
descriptor answers what the endpoint is; the registry answers what it will accept
today.

The host takes the endpoint's subscription directly rather than through
`async-nats`' micro service builder. That builder's error path publishes an empty
body, and 0016 requires the error body to be one complete `google.rpc.Status`;
the host cannot satisfy both. It therefore sets `Nats-Service-Error-Code` and
`Nats-Service-Error` itself, per section 5, and does not currently answer `$SRV`
discovery. That is a gap in the operational surface, not in the wire contract.

### 2. Commands are core request/reply, never a JetStream submission

The command subject is a core NATS subject. It is not backed by a stream, and a
host does not acknowledge, redeliver, or dead-letter a command.

Durability belongs to the events, not the request. Persisting the request too
would create a second source of truth for "did this happen" that can disagree
with the event stream, and it would introduce a redelivery loop whose retries the
original caller cannot observe. A caller that needs durable submission puts its
own work queue in front of the host and drives this binding from its consumer;
that keeps the retry policy with the party that knows the business deadline.

### 3. The surface is a protobuf service with one method

The wire contract is `trogonai.decider.v1.DeciderService`, declared in
`proto/trogonai/decider/v1/decider_service.proto` and annotated with
`trogon.nats.micro.v1alpha1.service`. Per
[ADR#0016](./0016-protobuf-rpc-over-nats-micro-binding.md) the proto `service` is
the canonical contract, so a client derives the subject, the request type, the
response type, and the error convention from the descriptor rather than from this
document.

The service declares one `rpc`, and therefore exposes one endpoint:

```text
{prefix}.DeciderService.Decide
```

`prefix` is the configured subject namespace, a dotted NATS token defaulting to
`decider`, which yields `decider.DeciderService.Decide`.

**One method rather than one per command type.** 0016 derives an endpoint from
each `rpc`, which requires the method set to be known when the schema is written.
A decider's command set is not: it is whatever the modules activated at run time
declare. Naming the command inside the request is what lets a fixed descriptor
carry an open set, and it is what makes runtime activation work without a
subscription-management protocol. A host with one subscription per command type
would have to add and drop subscriptions in lockstep with every `activate` and
`retire`, and a rollout would have a window in which the routing table and the
subscription set disagree.

**The subject names the service, never the module and never the command.** A
module version swap is invisible to callers by design (see
`DeciderRegistryHandle`'s rollout semantics), and putting the module or the
command in the subject would publish either a deployment detail or an open set as
part of the caller's contract.

The endpoint subject is checked at startup against
[ADR#0055](./0055-nats-subject-design-jsonrpc-bindings.md)'s shape and limit rules
with `trogon_nats::validate_published_subject`. A host whose configured prefix
pushes the subject past those limits can never receive a command, so it refuses to
start rather than serving a subject nobody can reach. The casing rule does not
apply: per 0016 the terminal is the method name verbatim, PascalCase included.

### 4. The request is one `DecideRequest`

The request payload is a `trogonai.decider.v1.DecideRequest` with
`Content-Type: application/protobuf`, which is the request type the endpoint's
method declares, so 0016's invariant that the request type is a property of the
endpoint holds.

| Field | Meaning |
| --- | --- |
| `command` | The command itself as a `google.protobuf.Any`. Required. |
| `command_id` | UUID identifying **this command**, not this delivery attempt. Makes retries idempotent (see section 6). Optional. |
| `expected_revision` | The stream revision the caller believes it is acting on. Strengthens the module's declared write precondition; can never weaken it. Optional, and never zero. |

`Any` rather than a type-and-bytes pair, because that pair is what `Any` already
is. Which module owns a command is resolved from `command.type_url`, and the host
never decodes `command.value`: it pairs the type with the bytes to build the WIT
`command-envelope`, because only the guest knows the schema behind them.

A `command_id` that is present and not a UUID fails the command rather than being
ignored: a caller whose idempotency key the host dropped would believe it has a
guarantee it does not have. An `expected_revision` of zero is likewise refused.
Zero would mean "I expect no events", which is the module's
`WritePrecondition::NoStream` to declare rather than a revision for a caller to
assert, and accepting it would let a caller express a guard the module never
agreed to.

Trace context propagation follows
[ADR#0042](./0042-nats-trace-context-and-message-path-tracing.md).

### 5. A reply is a `DecideResponse` or a service error

There are exactly two reply shapes, and
[ADR#0016](./0016-protobuf-rpc-over-nats-micro-binding.md) already decides which
one a caller is holding: **a reply is an error if, and only if,
`Nats-Service-Error-Code` is present.** This binding adds no discriminant of its
own, because a second one would be free to disagree with the first.

An **accepted** command answers with the method's response type,
`trogonai.decider.v1.DecideResponse`, which is the acceptance itself: the
`stream_position` the append reached and the events it appended. There is no
wrapper message between the response and what it reports, because the response
type already means "the command was decided and its events appended".

**Every other outcome**, a module's own refusal included, answers on the error
channel: one complete `google.rpc.Status` as the body, with `Status.code`
mirrored to `Nats-Service-Error-Code` and `Status.message` to
`Nats-Service-Error`. Per 0016 the headers are authoritative on any
disagreement, so the host builds both from the one `Status` value rather than
deriving them independently.

That is the ordinary contract of a protobuf RPC and the reason this binding
adds nothing to it: the response type is the shape of a success, and a caller
that did not get one reads a status. A response type carrying a second arm for
a refusal would be an error union bolted onto a channel that already has one,
and every caller would have to check both places to learn the one thing it
asked.

0016 section 3 leaves this to each specializing ADR, and the cost of deciding
it this way is real and accepted: a module refusing commands all day raises
micro's `num_errors`, so `num_errors` on a decider endpoint counts refused
commands alongside broken ones. An operator separates the two off
`ErrorInfo.domain`, which names the module for a refusal and
`trogonai.decider.v1` for anything the host itself decided. Paying for that
separation with a discriminant no other protobuf client would look for costs
every caller more than it costs the one operator reading the counter.

The host still records the finer `decision_outcome` telemetry attribute
(`decided`, `rejected`, `faulted`, `shed`, `denied`) on its own spans. That is a
trace vocabulary and not a wire one: a caller reads the error header and the
status, never a discriminant the host also publishes.

**Every outcome but an acceptance is a `google.rpc.Status`**, per 0016 section 3:
a canonical `google.rpc.Code`, a human-readable message, and standard `google.rpc`
detail messages in `details`.

Every status carries a `google.rpc.ErrorInfo`. Its `reason` is the stable,
machine-readable identifier a caller branches on, and its `domain` says who owns
that reason's namespace: `trogonai.decider.v1` for anything the host decided, and
the **module's own name** for a rejection, whose code space belongs to the module
rather than to the host. Without that split, two modules choosing the same
rejection code would be indistinguishable on the wire.

`details` is a `repeated google.protobuf.Any` with no ordering guarantee. A
reader locates a detail by unpacking on its type URL, never by indexing.

The response carries the appended events themselves, as `google.protobuf.Any`
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
`stream_position` it carries is the high watermark of the append as a
whole, not a position per event, and per `AppendStreamResponse`'s own contract it
is neither a next-expected-version nor safe to do arithmetic on. It is carried so
a caller can read its own write, and nothing more.

`Any` rather than a type-and-bytes pair, because that pair is what `Any` already
is. The one difference from the form the events carry on the stream is the
`type.googleapis.com/` prefix that the `Any` encoding requires and the
`Trogon-Event-Type` header does not; the host applies it at the reply boundary
and nowhere else.

A rejection is a service error whose code space is the module's. It is
`FAILED_PRECONDITION` rather than `INVALID_ARGUMENT`, which is the distinction
`google.rpc.Code` already draws: the command is well-formed and would succeed
against a different stream state. What separates it from every other status
here is its `ErrorInfo`, whose `domain` is the module and whose `reason` is the
module's own code, so a caller branching on a domain rule branches on the
module's vocabulary rather than on one the host invented for it.

A shed command is a service error for a different reason, and the difference is
worth keeping legible: per
[ADR#0028](./0028-decider-admission-control-and-backpressure.md) a shed command
was refused before anything was read, decided, or appended, so nothing about the
command was ever wrong. The host is full, not broken, and the
caller's correct response is backoff rather than escalation. It is
`RESOURCE_EXHAUSTED` and carries a `google.rpc.QuotaFailure` naming the limit it
contended for as a number, so the backoff can be sized rather than guessed. The
violation's `subject` is empty: the limit is the host's, so nothing about who
asked determined that the answer was no.

A denial is a service error for the same reason. Per
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

Every service error is a single `Status` whose class is read off
`ErrorInfo.reason`. A
reason rather than a nested `oneof`, because a code space is what a caller
branches on and `google.rpc` already publishes the convention for one; a
per-class message would also be a second discriminant alongside the code, free to
disagree with it. Two classes share `INTERNAL` on purpose: to a caller they are
one thing, and the reason is what tells an operator which party is answerable.

Every host-owned row below is also declared in
`proto/trogonai/decider/v1/faults.proto` as a `trogon.error.v1alpha1` template,
so the code and reason a caller should expect are readable off the schema
rather than only out of this table. Those messages are schema-only and never
encoded; the wire carries the `google.rpc.Status` it already did. The
`Rejected` row has no template because its domain and reason belong to the
module, not to the host, which is the one thing a host-owned schema cannot
declare on the module's behalf.

The mapping from the runtime's error taxonomy is total:

| `WasmCommandError` | Reply | `google.rpc.Code` | `ErrorInfo.reason` | Retry |
| --- | --- | --- | --- | --- |
| `Rejected` | service error | `FAILED_PRECONDITION` | the module's own code, under the module's domain | No; the command is wrong, not the moment |
| `Overloaded` | service error | `RESOURCE_EXHAUSTED` | `ADMISSION_LIMIT_REACHED` | Yes, after backoff |
| `Unauthorized::MissingPrincipal` | service error | `UNAUTHENTICATED` | `PRINCIPAL_MISSING` | No; not without credentials |
| `Unauthorized::Denied` | service error | `PERMISSION_DENIED` | `PRINCIPAL_UNAUTHORIZED` | No; not by this principal |
| unrouted command type | service error | `UNIMPLEMENTED` | `COMMAND_TYPE_UNROUTABLE` | Only after a module claiming it activates |
| undecodable request, unparseable field | service error | `INVALID_ARGUMENT` | `COMMAND_REQUEST_MALFORMED` | No |
| `PreconditionConflict` | service error | `INVALID_ARGUMENT` | `EXPECTED_REVISION_UNSATISFIABLE` | No; no stream state satisfies the revision the caller asserted |
| optimistic concurrency conflict | service error | `ABORTED` | `STREAM_WRITE_CONFLICT` | Yes; a retry replays the stream as it now stands |
| `Faulted`, `Trap`, `EmptyDecision`, `Evolve`, `StreamId`, `Instantiate` | service error | `INTERNAL` | `GUEST_FAULT` | No; a retry repeats the same guest call |
| `DeadlineExceeded` | service error | `DEADLINE_EXCEEDED` | `GUEST_DEADLINE_EXCEEDED` | Only once the load that slowed the guest has passed |
| `ReadSnapshot`, `ReadStream`, `Append` | service error | `UNAVAILABLE` | `STORAGE_UNAVAILABLE` | Yes, once storage recovers |
| `ReplayLimitExceeded`, `SnapshotAheadOfStream`, `ReadAfterOverflow`, `Blocking` | service error | `INTERNAL` | `HOST_INTERNAL` | No |

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

`DecideRequest.command_id` identifies the command, so the same value must be
reused across retries of one logical command and must differ between distinct
commands.
The host turns it into a `CommandId`, from which the execution derives each
event's id deterministically, which becomes the event's `Nats-Msg-Id` and is
deduplicated by the events stream. A retry of a command that already appended its
events therefore appends nothing the second time.

Absent the field, event ids are freshly generated and a retry appends a second
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
subject is one value for every command this host serves, so recording it
alongside `command_type` would say nothing `command_type` does not.

## Invariants

- A responder covers its command surface with one subscription, on the endpoint
  subject the descriptor names; per-command-type subscriptions are never used,
  because they cannot track runtime activation.
- The command type is read from `DecideRequest.command.type_url`. The host never
  decodes `command.value`.
- Neither the module name, the module version, nor the command type appears in
  the endpoint subject.
- A reply is an error if, and only if, `Nats-Service-Error-Code` is present. A
  reply is never both a `DecideResponse` and a `Status`, and an outcome is never
  signalled by an absent or malformed reply.
- On a service error, the headers and the body are built from one `Status` value,
  so they cannot disagree about what happened.
- A `DecideResponse` describes an acceptance and nothing else. Every other
  outcome, a domain rejection included, is a service error.
- Every outcome but an acceptance is a `google.rpc.Status` carrying a canonical
  `google.rpc.Code` and a `google.rpc.ErrorInfo`. The host defines no error
  message of its own and no such `Status` reports `OK`.
- `ErrorInfo.domain` is `trogonai.decider.v1` for everything the host decided and
  the module's name for a rejection. A host never reports a module's rejection
  code under its own domain.
- A caller reads `Status.details` by unpacking on a type URL. Nothing about the
  order or the count of that list is promised.
- A present-but-unparseable `command_id`, or an `expected_revision` of zero,
  fails the command; neither is ever ignored.
- Commands are core NATS. A host never acknowledges, redelivers, or dead-letters
  a command.

## Alternatives Considered

### One `rpc` per command type

Rejected, and it is the design 0016 would suggest if the command set were known
when the schema is written. It is not. A method per command type puts the
routable set in the descriptor, which means regenerating and redeploying the
schema to activate a module, and a rollout window in which the descriptor and the
registry disagree. `Any` in the request is what keeps the descriptor static while
the set behind it moves.

### Put the command type in the subject instead of the request body

Rejected, and this is the design an earlier draft of this ADR adopted. Its
argument was subject-prefix authorization: `decider.trogonai.scheduler.schedules.v1.>`
is a grantable capability meaning exactly "the scheduler's commands", which one
subject per service cannot express.

The cost is that it is not the 0016 binding. A generated client cannot reach it,
because nothing in a descriptor says the subject encodes the request type; every
caller needs a hand-written client and a copy of the subject rule. Paying that on
every caller, forever, to move one authorization decision from the host to the
broker is the wrong trade. Per-command authorization is the host's under
[ADR#0026](./0026-command-authorization-principal.md), where the principal is
already known and the module's own rules are already being applied.

### Carry commands on a JetStream work queue with durable consumers

Rejected per section 2: it duplicates the durability the event stream already
provides and hides retries from the caller. A deployment that wants it composes
it in front of this binding, which keeps the retry policy where the deadline is
known.

### Model every outcome as one `oneof` and keep the error channel unused

Rejected, and this too is what an earlier draft adopted: a five-arm
`CommandOutcome` with `decided`, `rejected`, `faulted`, `shed`, and `denied`,
plus a `Trogon-Decider-Outcome` header carrying the same discriminant.

It reads well and it is wrong on 0016. A reply that never sets
`Nats-Service-Error-Code` is a reply every micro-aware intermediate reads as a
success, so a host that faulted on every request would report `num_errors: 0` and
look healthy. The header was a second discriminant free to disagree with the
body, which is the thing 0016 settles by making the header authoritative. And the
supposed benefit, an exhaustive match, is only exhaustive for callers that decode
the body, which is exactly the set of callers that did not need the header.

### Keep a `rejected` arm in the response body

Rejected, and this is what an earlier draft of this ADR adopted, on the reading
of 0016 section 3 that a refusal executed successfully and therefore belongs in
the typed response. The reading is defensible and the design is still worse.

It makes `DecideResponse` an error union in front of a channel that already is
one, so a caller cannot learn the outcome from the status it already has to
handle: it decodes the body, matches an arm, and finds a `google.rpc.Status`
inside a message it decoded to avoid one. A generated client gets none of that
for free, because nothing in the descriptor says one arm of a response is an
error. And it splits `google.rpc.Status` across two channels, which is the
thing that makes a caller's error handling non-uniform: the same type, in two
places, meaning the same thing.

What it bought was `num_errors` not counting refusals. That is one counter on
one endpoint, recoverable from `ErrorInfo.domain`, and it is not worth making
every caller of this service read replies differently from every other
protobuf service they call.

### Define per-outcome error messages instead of `google.rpc.Status`

Rejected. 0016 fixes the shape of an error platform-wide; what this binding adds
is only which channel it arrives on.

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
  a decider is. Nothing about failure is decider-specific except which domain
  owns the reason.
- `num_errors` on a decider endpoint counts refused commands alongside broken
  ones. An operator who needs the two apart splits them on `ErrorInfo.domain`
  rather than on the counter.
- Adding a fault class is now a new `ErrorInfo.reason` under an existing code,
  not a new message and not a `oneof` arm. Callers that branch on the code keep
  working; only the ones that branch on the reason see it.
- Callers that want at-least-once submission must build it. This binding gives
  them the idempotency key that makes it safe; it does not give them the queue.
- `decision_outcome` is now telemetry only. It no longer has to match anything on
  the wire, so an outcome can be split for an operator's benefit without being a
  breaking change for a caller.
- An authorization policy written against the subject can no longer distinguish
  one command type from another, because there is one subject. Per-command
  authorization is the host's, via [ADR#0026](./0026-command-authorization-principal.md),
  and not the broker's.

## Related ADRs

- [ADR#0009: Protocol Buffers Wire Contracts](./0009-protocol-buffers-wire-contracts.md)
- [ADR#0016: Protocol Buffers RPC over NATS micro Binding](./0016-protobuf-rpc-over-nats-micro-binding.md)
- [ADR#0028: Decider Admission Control and Backpressure](./0028-decider-admission-control-and-backpressure.md)
- [ADR#0042: NATS Trace Context and Message Path Tracing](./0042-nats-trace-context-and-message-path-tracing.md)
- [ADR#0045: Aggregate-Oriented Module Layout for Event-Sourced Services](./0045-event-sourced-service-module-layout.md)
- [ADR#0055: NATS Subject Design for JSON-RPC Protocol Bindings](./0055-nats-subject-design-jsonrpc-bindings.md)

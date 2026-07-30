# Event Metadata

Event payloads are the canonical domain facts used for replay, projections, and
business behavior. Event headers are envelope metadata for operational context
such as correlation, [tenancy](../glossary/tenant), causation, or [transport](../glossary/transport) routing.
Recorder-assigned time belongs to the persisted event envelope as `recorded_at`,
not to individual domain payloads.

The [decider](../glossary/decider) runtime should not derive required headers from [commands](../glossary/command) or emitted
events through a generic callback. If a workflow requires a fixed header set,
make that requirement explicit before command execution with a typed input owned
by the application boundary, validate it there, then pass the validated
`Headers` into `CommandExecution::with_headers`.

This keeps metadata policy close to the caller that knows why the metadata is
mandatory, while the runtime stays responsible for execution and storage
adapters stay responsible for persisting event envelopes.

Trace context is one class of operational event metadata. See
[OpenTelemetry Transport Context](./opentelemetry-transport-context.md) for the
logical and physical carrier mappings and their current implementation status.

## Trace context lifecycle

Trace metadata is immutable with the event. The NATS event-store adapter may
encode logical `traceparent` as physical `Trogon-Header-traceparent`, but reading
it decodes the same name and value. Replay, projection rebuild, consumer
recovery, and every form of restoration keep the stored context of an existing
event and must not rewrite it.

Here, replay means reading already persisted events back into an aggregate or
projection. It performs no append, so there is no header creation or rewrite.
Restoration splits by layer. Native JetStream snapshot restore reconstructs
stored messages, so it also performs no application append. An application that
restores offloaded or archived events appends them into a replacement stream and
therefore does create a new NATS message for each one, as
[ADR#0013](../adr/0013-origin-stream-sequence-header.md) describes. That append
carries the restore job's own message context and still leaves the restored
event's stored context unchanged.

Import and republish differ only when an application crosses a new creation
boundary. Native JetStream snapshot restore reconstructs the stored message and
headers. Native source or mirror replication and server-side `RePublish`
preserve the existing direct NATS `traceparent` while NATS adds its own
provenance headers. None of those server operations creates an OpenTelemetry
producer span or a new message context.

That preservation of the direct carrier holds on NATS 2.14 or newer. Earlier
NATS ADR-41 servers can rewrite `traceparent` at storage, sampling, or account
boundaries, so a mixed or older topology cannot treat it as a guarantee, and
those servers do not belong on a path that relies on direct W3C propagation. The
stored event metadata is unaffected either way, because it is durable event data
rather than the live NATS carrier.
[ADR#0042 (Draft)](../adr/0042-nats-trace-context-and-message-path-tracing.md)
proposes that version floor.

An application that consumes a historical event and publishes a new NATS
message does create a new message boundary. The new message receives the active
direct `traceparent`, while the embedded historical event metadata remains
unchanged. [ADR#0013](../adr/0013-origin-stream-sequence-header.md) defines the
application-level origin-sequence metadata when an event is appended into a
different stream.
[ADR#0042 (Draft)](../adr/0042-nats-trace-context-and-message-path-tracing.md)
proposes the NATS trace carrier and server-operation boundary.

An import that transforms source data into a new domain event gives the new
event its own creation context. The import span may link the source context, but
no accepted ADR reserves a second durable source-trace header. Adding one would
require a separate metadata decision; do not overload the historical event's
`traceparent` with the new operation context.

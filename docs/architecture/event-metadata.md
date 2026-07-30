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
recovery, and restoration from offloaded storage do not create a new event and
must not rewrite that context.

Here, replay means reading already persisted events back into an aggregate or
projection. It performs no append, so there is no header creation or rewrite.

Import and republish differ only when an application crosses a new creation
boundary. Native JetStream snapshot restore reconstructs the stored message and
headers. Native source or mirror replication and server-side `RePublish`
preserve the existing direct NATS `traceparent` while NATS adds its own
provenance headers. None of those server operations creates an OpenTelemetry
producer span or a new message context.

An application that consumes a historical event and publishes a new NATS
message does create a new message boundary. The new message receives the active
direct `traceparent`, while the embedded historical event metadata remains
unchanged. [ADR#0013](../adr/0013-origin-stream-sequence-header.md) defines the
application-level origin-sequence metadata when an event is appended into a
different stream.
[ADR#0041 (Draft)](../adr/0041-nats-trace-context-and-message-path-tracing.md)
proposes the NATS trace carrier and server-operation boundary.

An import that transforms source data into a new domain event gives the new
event its own creation context. The import span may link the source context, but
no accepted ADR reserves a second durable source-trace header. Adding one would
require a separate metadata decision; do not overload the historical event's
`traceparent` with the new operation context.

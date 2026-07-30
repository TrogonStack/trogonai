---
number: "0013"
slug: origin-stream-sequence-header
status: accepted
date: 2026-06-29
---

# ADR#0013: Origin Stream Sequence Header

## Context

[JetStream](../glossary/jetstream) assigns each stored message a [stream](../glossary/stream) sequence. [Processors](../glossary/processor) and
[projections](../glossary/projection) use the current JetStream stream sequence as the physical position of
an [event](../glossary/event) in the stream they are consuming. That position is the correct cursor
for [checkpoints](../glossary/checkpoint), high-water marks, and replay progress within the current stream.

Some application-level [event-sourcing](../glossary/event-sourcing) workflows can append an event into a
different stream from the one where it originally lived. Examples include
restoring archived events outside native JetStream snapshot restore, backfilling
historical events, rebuilding a stream after a topology change, or migrating
events between stream layouts.

In those workflows, JetStream assigns a new stream sequence in the destination
stream. The current sequence still identifies where the event lives now, but it
no longer tells a projection, repair tool, or diagnostic workflow where the event
originally lived.

[NATS](../glossary/nats) has server-owned [headers](../glossary/headers) such as `Nats-Sequence` for republish and direct
get contexts, and reserves the `Nats-` header namespace. Application-owned event
metadata must not use that namespace.

## Decision

Reserve the event metadata header `Trogon-Origin-Stream-Sequence`.

The header records the original JetStream stream sequence of an event when that
event is written into a stream other than the stream position where it originally
lived.

This is the logical event-header name. The NATS event-store adapter applies its
generic physical mapping, so the stored NATS header is
`Trogon-Header-Trogon-Origin-Stream-Sequence`. Reads remove only the adapter
prefix and return the logical name above. Application code and projections use
the logical name rather than the physical storage spelling.

Set `Trogon-Origin-Stream-Sequence` when:

- Restoring archived events through an application that appends them into a
  replacement or recovery stream.
- Backfilling historical events into a live or rebuilt stream.
- Migrating events into a different stream topology.
- Rebuilding a stream while preserving provenance for projection verification,
  diagnostics, or repair tooling.

Do not add this header during native JetStream snapshot restore. A native
restore reconstructs the stored stream data, including the original message
headers and sequence state. Native source and mirror replication may add the
server-owned `Nats-Stream-Source` header, and server-side `RePublish` adds its
own origin headers. Those server mechanisms do not require an application to
synthesize this header.

Do not set `Trogon-Origin-Stream-Sequence` during ordinary event appends. During
normal operation, the current JetStream message metadata already provides the
stream sequence.

The current JetStream sequence values remain authoritative for processors,
consumers, checkpoints, high-water marks, and optimistic concurrency decisions
owned by the NATS adapter.

Consumers must not use this header for JetStream consumption tracking,
acknowledgements, resume position, or ordinary processor high-water marks.

`Trogon-Origin-Stream-Sequence` is provenance metadata. It is not an optimistic
concurrency token, not a checkpoint cursor by default, and not a per-aggregate
version.

When the header is absent, processors and projections must use the current
JetStream stream sequence as usual.

## Projection Provenance Rule

For projection provenance, rebuild verification, diagnostics, and repair
tooling, derive an effective origin stream sequence as follows:

- Use `Trogon-Origin-Stream-Sequence` when present.
- Otherwise use the current JetStream stream sequence from the consumed message
  metadata.

This rule is only for origin provenance. It does not change checkpoint,
acknowledgement, resume, high-water mark, or optimistic concurrency behavior.

For example, a restored event may be consumed from a replacement stream at
JetStream stream sequence `9500` and carry
`Trogon-Origin-Stream-Sequence: 120`. The projection checkpoints `9500` because
that is the position it consumed. The projection may record or compare `120` as
the event's original stream position for provenance, rebuild verification,
diagnostics, or repair.

## Consequences

- Event replay, restore, migration, and repair workflows can preserve the origin
  stream position without confusing it with the destination stream position.
- Projection diagnostics can compare current stream order with original stream
  order when an event has been rebuilt or restored.
- Checkpointing remains tied to the current stream, so processors can resume
  using the stream they actually consume.
- Application metadata stays out of the reserved `Nats-` header namespace.
- The ADR intentionally does not define archive format, stream generation,
  aggregate versions, or a complete event identity model.

## References

- [ADR#0004: Protocol and Transport Layering](./0004-protocol-and-transport-layering.md)
- [ADR#0010: Unit Tests First, Testcontainers Only When Necessary](./0010-testcontainers-for-infrastructure-tests.md)
- [ADR#0041: NATS Trace Context and Message Path Tracing (Draft)](./0041-nats-trace-context-and-message-path-tracing.md)
- [NATS JetStream Headers](https://docs.nats.io/nats-concepts/jetstream/headers)
- [NATS JetStream Source and Mirror Streams](https://docs.nats.io/nats-concepts/jetstream/source_and_mirror)
- [NATS JetStream Disaster Recovery](https://docs.nats.io/running-a-nats-service/nats_admin/jetstream_admin/disaster_recovery)

---
number: "0042"
slug: nats-trace-context-and-message-path-tracing
status: draft
date: 2026-07-30
---

# ADR#0042: NATS Trace Context and Message Path Tracing

## Context

[ADR#0008](./0008-opentelemetry-observability.md) selects W3C context
propagation and OpenTelemetry for first-party observability.
[ADR#0004](./0004-protocol-and-transport-layering.md) assigns transport
headers and transport telemetry to transport adapters, while
[ADR#0018](./0018-connectrpc-gateway-for-browser-product-surfaces.md)
requires browser context to continue from a Connect gateway onto NATS. Those
decisions do not settle how a NATS carrier interacts with NATS server message
path tracing, JetStream storage, account boundaries, or durable event metadata.

NATS ADR-41 gives the exact, direct `traceparent` header two related roles. It
continues the W3C message context for application instrumentation, and its
sampled flag can activate NATS message path tracing when the publishing account
has `msg_trace` configured. NATS then emits server trace events for ingress,
subject mapping, account imports or exports, JetStream, and egress. That path
model includes routes, gateways in a supercluster, and leaf nodes.

The two trace systems are complementary, not interchangeable. OpenTelemetry
application spans are exported through OTLP. NATS message path events are NATS
JSON events published to a configured subject. NATS does not turn those events
into OpenTelemetry spans or export them to an OpenTelemetry backend.

The distinction between forwarding and creation is also important. A server
route, gateway, leaf connection, JetStream delivery, source or mirror, and
server-side `RePublish` move an existing NATS message without starting an
application producer span. Native JetStream backup and restore preserve the
stored message and its headers. An application that consumes data and publishes
a new message does cross a new message-creation boundary.

NATS versions that first implemented ADR-41 could rewrite `traceparent` to
`Xraceparent` when tracing was disabled at storage, sampling, or account
boundaries. NATS 2.14 stopped rewriting the W3C header. A local version policy
is therefore required before direct W3C propagation can be treated as stable
through a mixed server topology.

## Decision

### 1. Use the official direct NATS carrier

NATS Core and JetStream application messages carry W3C context in direct,
lowercase NATS headers named `traceparent` and `tracestate`. They carry
`baggage` only when the active propagator and boundary policy permit baggage.

`traceparent` follows W3C Trace Context and NATS ADR-41. It is not a local
Trogon header. `tracestate` and `baggage` remain application headers that NATS
forwards without assigning them broker-tracing semantics.

The `Nats-` namespace remains server-owned. Applications must not set internal
headers such as `Nats-Trace-Hop` or `Nats-Trace-Origin-Account`.

### 2. Keep application, broker, and event context distinct

The platform recognizes three different records of tracing context:

- Direct NATS W3C headers describe the creation context of the current NATS
  message for application OpenTelemetry instrumentation.
- NATS message path events describe how that message traversed the NATS
  topology.
- Logical durable event `Headers` describe the creation context stored with an
  immutable event. The NATS event-store adapter physically encodes those names
  as `Trogon-Header-{name}` and decodes them back to the original logical names.

A message may therefore contain both `traceparent` and
`Trogon-Header-traceparent`. The first belongs to the current NATS message. The
second belongs to the durable event carried by that message. Neither header is
a substitute for NATS path events.

### 3. Preserve context through NATS server operations

NATS server forwarding through routes, gateways, leaf nodes, JetStream storage
and redelivery, native source or mirror replication, server-side `RePublish`,
and native snapshot restore are preservation operations. They keep the
message's existing direct W3C carrier and do not invent a new application trace
context.

NATS may add its own provenance headers. In particular, server-side
`RePublish` adds headers such as `Nats-Stream`, `Nats-Subject`, and
`Nats-Sequence`, while a source adds `Nats-Stream-Source`. Those server headers
do not replace application event provenance defined by
[ADR#0013](./0013-origin-stream-sequence-header.md).

An application consume-and-publish workflow creates a new NATS message. Its
producer or client span injects the new direct W3C context. If the payload is an
existing durable event, that event's logical trace metadata remains unchanged
and can coexist with the new direct carrier.

### 4. Link asynchronous consumption and replay context

Generic NATS receive and process spans follow the OpenTelemetry messaging
default: keep the current operation context as the parent and link each
message's extracted creation context. An instrumentation may use the message
context as the parent only for a documented, single-message process path. If it
uses that opt-in while a valid ambient context exists, it links the ambient
context.

When NATS carries a higher-level protocol with its own carrier, such as MCP
`params._meta`, the protocol context is the remote parent of the protocol server
span. The direct NATS context represents the transport message and is linked
rather than substituted for the protocol parent.

A live durable-event consumer links the event's stored creation context to its
process span. When the direct message context and stored event context are both
present and distinct, it links both. A replay, projection rebuild, repair, or
restore job keeps its own operation context as the parent and links historical
event contexts. It must not make an old producer span the parent of a later
operational job or write the job's current context back into a historical
event.

### 5. Treat NATS message path tracing as an explicit operation

Account-level `msg_trace` remains disabled unless a deployment explicitly
defines all of the following:

- a trace destination subject;
- sampling appropriate to expected traffic;
- publish and subscribe permissions for the destination;
- retention and size limits for collected events; and
- access controls appropriate for topology, subject, account, server, and
  client details disclosed by the events.

Ad hoc `Nats-Trace-Dest` and `Nats-Trace-Only` are diagnostic controls, not the
normal application propagation carrier. `Nats-Trace-Dest` takes precedence over
trace-context activation. `Nats-Trace-Only` must be used only when intentionally
preventing application delivery and JetStream storage.

Broker tracing stops at account boundaries by default. `allow_trace: true` may
be enabled on an import or export only after the owners of both sides accept the
additional topology visibility. This setting controls NATS path-event
visibility. It does not authorize baggage propagation or change application
OpenTelemetry parentage.

### 6. Require a compatible NATS topology

Every NATS server on a path that relies on direct W3C propagation, including
JetStream servers and servers connected by routes, gateways, or leaf links,
must run NATS 2.14 or newer. Servers from the earlier ADR-41 implementation are
not permitted on that path because they can rewrite `traceparent` at storage,
sampling, or account boundaries.

NATS 2.14 may use the broker control `Nats-Trace-Dest: trace disabled` instead
of rewriting `traceparent` when broker path tracing should not continue.
Application instrumentation must ignore that control when extracting W3C
context. Preserving application context does not promise that NATS path-event
collection continues past the same boundary.

The repository must not enable trace-context-driven `msg_trace` in a mixed or
unverified topology. Conformance tests must cover the deployed server version
and the relevant route, gateway, leaf, JetStream, source or mirror, republish,
and account-boundary paths. A NATS 2.10 test fixture cannot verify ADR-41
behavior because message path tracing was introduced in NATS 2.11.

### 7. Do not claim broker spans without an explicit bridge

NATS message path events remain NATS events. If the platform later needs NATS
server hops in an OpenTelemetry backend, a dedicated adapter must define event
collection, DAG reconstruction, span identity and parentage, timestamp mapping,
sampling reconciliation, redaction, and OTLP export. Until that contract exists,
documentation and product surfaces must not describe NATS path events as
OpenTelemetry spans or promise one combined application-and-broker span tree.

## Consequences

- The live NATS carrier uses the official `traceparent` name rather than a
  Trogon-specific alias.
- `Trogon-Header-traceparent` remains valid for durable event metadata because
  it serves a different lifecycle and is decoded back to logical
  `traceparent`.
- Server-side republish, source or mirror replication, backup, restore, and
  redelivery preserve message context. Only application message creation
  injects a new direct context.
- Asynchronous consumers and later replay jobs correlate through links without
  changing immutable event context or forcing an old request to parent a new
  operation.
- A supercluster can expose a broker path across gateways, but it does not make
  a JetStream replica set span clusters. Streams and their replicas remain in
  the stream's placement cluster; cross-cluster copies use source, mirror, or
  explicit application workflows.
- NATS 2.14 becomes the minimum supported server version for an ADR-41-aware
  topology that must preserve direct W3C context.
- Enabling broker path tracing adds operational cost and exposes topology
  details, so it remains a deployment choice rather than an automatic side
  effect of installing OpenTelemetry.

## References

- [ADR#0004: Protocol and Transport Layering](./0004-protocol-and-transport-layering.md)
- [ADR#0008: OpenTelemetry Observability](./0008-opentelemetry-observability.md)
- [ADR#0013: Origin Stream Sequence Header](./0013-origin-stream-sequence-header.md)
- [ADR#0018: ConnectRPC Gateway for Browser Product Surfaces](./0018-connectrpc-gateway-for-browser-product-surfaces.md)
- [OpenTelemetry Transport Context](../architecture/opentelemetry-transport-context.md)
- [NATS ADR-41: NATS Message Path Tracing](https://github.com/nats-io/nats-architecture-and-design/blob/main/adr/ADR-41.md)
- [NATS JetStream Headers](https://docs.nats.io/nats-concepts/jetstream/headers)
- [OpenTelemetry Messaging Span Context Propagation](https://opentelemetry.io/docs/specs/semconv/messaging/messaging-spans/#context-propagation)
- [NATS JetStream Streams and Placement](https://docs.nats.io/nats-concepts/jetstream/streams#placement)
- [NATS 2.14 traceparent preservation change](https://github.com/nats-io/nats-server/pull/7755)
- [NATS 2.14.2 source header preservation](https://github.com/nats-io/nats-server/blob/v2.14.2/server/stream.go#L4394-L4405)
- [NATS 2.14.2 server-side republish header handling](https://github.com/nats-io/nats-server/blob/v2.14.2/server/stream.go#L7040-L7073)
- [NATS JetStream Disaster Recovery](https://docs.nats.io/running-a-nats-service/nats_admin/jetstream_admin/disaster_recovery)

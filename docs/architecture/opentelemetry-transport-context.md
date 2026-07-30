# OpenTelemetry Transport Context

This reference records how trace context crosses each protocol, transport, and
durable event boundary in this repository. It distinguishes official carrier
contracts from local conventions and records the implementation as reviewed on
2026-07-30.

No current path provides end-to-end extraction and injection across every hop.
Some paths export spans, some preserve trace fields, and some inject an outbound
context, but those capabilities are not equivalent to a correlated distributed
trace.

## Authority and ownership

The following accepted decisions govern this area:

- [ADR#0003: AI Protocol Transport Taxonomy](../adr/0003-ai-protocol-transport-taxonomy.md)
  makes one OpenTelemetry service identity part of an operated workload.
- [ADR#0004: Protocol and Transport Layering](../adr/0004-protocol-and-transport-layering.md)
  assigns headers, connection lifecycle, and transport telemetry to transport
  adapters.
- [ADR#0008: OpenTelemetry Observability](../adr/0008-opentelemetry-observability.md)
  selects OpenTelemetry spans, context propagation, semantic conventions, and
  OTLP-compatible export for first-party observability.
- [ADR#0013: Origin Stream Sequence Header](../adr/0013-origin-stream-sequence-header.md)
  keeps application metadata out of the NATS-owned `Nats-` namespace.
- [ADR#0018: ConnectRPC Gateway for Browser Product Surfaces](../adr/0018-connectrpc-gateway-for-browser-product-surfaces.md)
  requires a browser `traceparent` to continue through a Connect gateway onto
  NATS.

For a protocol-level carrier, the protocol specification takes precedence. For
HTTP and generic text-map propagation, W3C and OpenTelemetry guidance applies.
Where upstream guidance requires propagation but does not define a physical
carrier, this page records the local carrier already represented by the code.

This page does not introduce new wire names or settle an unresolved lifecycle
policy. A new or incompatible cross-cutting policy requires an ADR. Implementing
an already selected official contract does not require one ADR per missing hop.

[ADR#0042: NATS Trace Context and Message Path Tracing (Draft)](../adr/0042-nats-trace-context-and-message-path-tracing.md)
proposes the remaining local NATS policy: version compatibility, broker path
tracing, account boundaries, server-side preservation, and the separation
between direct message context and durable event metadata. It is not binding
until accepted under the ADR process.

## Status vocabulary

| Status | Meaning |
| --- | --- |
| End to end | The receiver extracts remote context, uses it as a parent or link, and the next sender injects the resulting context. |
| Outbound only | A sender injects the current context, but the corresponding receive path does not extract it. |
| Pass-through only | A wire model preserves trace fields, but those fields are not connected to the OpenTelemetry context. |
| Instrumented only | The path creates or exports spans but does not extract or inject a remote carrier. |
| Carrier available | The boundary can store the fields when a caller supplies them, but does not capture them automatically. |
| Not OTel-enabled | The standalone runtime does not install the repository OpenTelemetry providers and propagator. |

## Shared runtime behavior

`trogon-telemetry` installs a global `TraceContextPropagator` after its
OpenTelemetry providers initialize successfully. The configured propagator
therefore handles `traceparent` and non-empty `tracestate`. It does not install a
W3C Baggage propagator, so `baggage` is reserved by MCP and ACP but is not
currently extracted or injected by repository telemetry code.

The same package exports traces, metrics, and logs over OTLP/HTTP. OTLP is the
signal export path to a collector or backend; it is not the carrier used to
continue application traces between services.

The following binaries initialize `trogon-telemetry`:

- `trogon-gateway`
- `acp-nats-server`
- `acp-nats-stdio`
- `mcp-nats-server`
- `mcp-nats-stdio`

Other binaries may create `tracing` spans, but those spans are not exported by
the repository OpenTelemetry pipeline unless the embedding process installs its
own subscriber and providers.

## Signal names and attributes

`otel/semconv/` is the repository's language-neutral inventory for current
metric names, span names, and attribute keys. Generated Rust constants live in
`trogon-semconv`. That registry records standard OpenTelemetry attributes and
local attributes as they are emitted today; it is not a trace carrier registry
or a claim of upstream conformance.

The current transport instrumentation has these known differences from the
linked upstream semantic conventions:

- `trogon-std` names every HTTP server span `http.server.request` rather than
  the recommended `{method} {route}` shape. It records several standard HTTP
  attributes, but does not record every required attribute or extract the
  incoming context.
- `trogon-nats` creates request and publish spans and records standard
  `messaging.*` attributes, but it has no receive or process instrumentation.
  The send spans do not set an explicit OpenTelemetry messaging span kind.
- `mcp-nats` records generic `send` and `receive` transport spans. It does not
  implement the OpenTelemetry MCP client and server span model or its
  `params._meta` parent and ambient-link behavior.

Local semantic-convention entries document those differences until the runtime
instrumentation is brought into alignment. They do not override an official
OpenTelemetry name, attribute, span-kind, or parentage requirement.

## Carrier contracts

| Boundary | Governing guidance | Carrier |
| --- | --- | --- |
| HTTP | [W3C Trace Context](https://www.w3.org/TR/trace-context/), [W3C Baggage](https://www.w3.org/TR/baggage/), and [OpenTelemetry propagators](https://opentelemetry.io/docs/specs/otel/context/api-propagators/) | HTTP headers named `traceparent`, `tracestate`, and, when baggage propagation is enabled, `baggage`. |
| MCP messages | [MCP SEP-414](https://modelcontextprotocol.io/seps/414-request-meta) and [OpenTelemetry MCP semantic conventions](https://github.com/open-telemetry/semantic-conventions-genai/blob/main/docs/gen-ai/mcp.md) | Unprefixed `traceparent`, `tracestate`, and `baggage` keys in request or notification `params._meta`. MCP context is independent from an underlying HTTP context. |
| ACP messages | [ACP v1 Extensibility](https://agentclientprotocol.com/protocol/v1/extensibility), [ACP Meta Field Propagation Conventions](https://agentclientprotocol.com/rfds/meta-propagation), and [ACP v1 Transports](https://agentclientprotocol.com/protocol/v1/transports) | Root keys `traceparent`, `tracestate`, and `baggage` in message `params._meta`. Custom transports preserve the ACP JSON-RPC message and lifecycle contract. |
| NATS Core and JetStream messages | [NATS ADR-41](https://github.com/nats-io/nats-architecture-and-design/blob/main/adr/ADR-41.md), [NATS JetStream headers](https://docs.nats.io/nats-concepts/jetstream/headers), and [OpenTelemetry messaging span conventions](https://opentelemetry.io/docs/specs/semconv/messaging/messaging-spans/) | The configured text-map propagator operates directly over NATS headers, producing `traceparent`, `tracestate`, and, when enabled, `baggage`. Exact direct `traceparent` also has official NATS broker-tracing semantics. |
| Durable decider events | [Event Metadata](./event-metadata.md) and the repository event-store adapter | Logical event `Headers` carry the W3C field names. The NATS adapter encodes each logical name physically as `Trogon-Header-{name}` and removes that prefix when reading. This mapping is lossless: reads, replays, and restores preserve the original names and values. |
| A2A protocol bindings | [A2A 1.0 specification](https://a2a-protocol.org/latest/specification/) and the standard propagation rules of the selected HTTP or gRPC binding | A2A 1.0 does not assign protocol metadata keys for W3C trace context. This repository has no A2A-specific carrier convention. |
| OTLP export | [OTLP specification](https://opentelemetry.io/docs/specs/otlp/) | OTLP/HTTP carries completed telemetry signals to the configured exporter endpoint. It does not continue the application request context. |

For MCP, the OpenTelemetry guidance is more specific than ordinary HTTP
instrumentation: inject the configured propagator into `params._meta`, extract
that context as the remote parent of the MCP server span, and link any ambient
transport context. One HTTP stream can carry multiple MCP messages, so an HTTP
server span alone cannot represent the MCP parentage.

For NATS and other asynchronous messaging, OpenTelemetry recommends span links
as the default correlation between a consumer operation and each message
creation context. A single-message process span may instead use that context as
its parent, but the instrumentation should document the choice and preserve any
valid ambient context with a link.

## NATS broker path tracing

NATS message path tracing and OpenTelemetry application tracing observe
different layers of the same message flow:

| Concern | Activation | Output |
| --- | --- | --- |
| Application trace propagation | Direct W3C headers injected and extracted by application instrumentation | Application spans exported through OTLP |
| Account-configured NATS path tracing | A valid direct `traceparent` with the sampled flag, plus account `msg_trace` configuration and its additional sampling | NATS JSON trace events published to the configured destination subject |
| Ad hoc NATS diagnostics | `Nats-Trace-Dest`, optionally with `Nats-Trace-Only` | NATS JSON trace events published to the requested subject |

NATS path events cover server ingress and egress, subject mapping, account
imports and exports, JetStream, routes, gateways, and leaf nodes. Events from
each server form a hop DAG. They are not OpenTelemetry spans and NATS does not
export them through OTLP. Showing broker hops in an OpenTelemetry backend would
require a separate adapter that reconstructs the DAG and defines span mapping,
sampling, redaction, and export.

`Nats-Trace-Dest` takes precedence over trace-context activation.
`Nats-Trace-Only: true` prevents final application delivery and JetStream
storage, so it is a diagnostic control rather than a propagation setting.
Cross-account broker tracing is denied by default and requires `allow_trace:
true` on the applicable import or export. That opt-in controls broker topology
visibility, not whether application protocol context is valid.

NATS 2.11 introduced path tracing, but its original implementation could rename
`traceparent` to `Xraceparent` at storage, sampling, or account boundaries. NATS
2.14 stopped modifying the W3C header and can use `Nats-Trace-Dest: trace
disabled` to suppress broker path tracing instead. Application extraction still
uses the preserved `traceparent`; the broker control does not become
OpenTelemetry context. A topology that relies on direct W3C propagation and
uses ADR-41 behavior should therefore run NATS 2.14 or newer on every server in
the message path, including JetStream, route, gateway, and leaf servers.

A supercluster extends Core NATS routing across cluster gateways. It does not
turn a JetStream stream into a cross-cluster replica set. A stream and all of
its replicas remain in one placement cluster. Cross-cluster durable copies use
sources, mirrors, native backup and restore, or application workflows, each
with the lifecycle distinctions documented below.

## Current implementation matrix

| Path | Current behavior | Status |
| --- | --- | --- |
| Shared `trogon-nats` request and publish helpers | `headers_with_trace_context` and `inject_trace_context` inject the current span context into outgoing NATS headers. There is no shared NATS header extractor. | Outbound only |
| NATS server path tracing | The development compose file uses NATS 2.14.2, which preserves `traceparent`, but no repository server configuration enables account `msg_trace` or cross-account `allow_trace`. The shared NATS integration fixture uses 2.10.14 and therefore cannot exercise NATS ADR-41, which was introduced in 2.11. | Development version compatible; shared ADR-41 coverage missing |
| MCP over NATS | `mcp-nats` injects the current context on requests, notifications, responses, and errors. Incoming NATS headers supply MCP transport routing metadata but are not extracted into OpenTelemetry. `params._meta` travels verbatim in the canonical JSON-RPC body defined by [ADR#0041 (Draft)](../adr/0041-canonical-mcp-jsonrpc-bodies-over-nats.md), and the resolved `rmcp` dependency exposes SEP-414 accessors, but repository code neither populates those fields from the active context nor extracts them as a remote parent. | Outbound only plus pass-through only |
| MCP Streamable HTTP | `mcp-nats-server` initializes OpenTelemetry, but its Axum service does not use the repository HTTP instrumentation or another repository-owned W3C extractor. The MCP model preserves `_meta`; no code connects it to OpenTelemetry before forwarding work to NATS. | Pass-through only |
| MCP stdio | `mcp-nats-stdio` initializes OpenTelemetry and forwards typed MCP messages. Stdio has no transport headers, so SEP-414 `_meta` is the applicable carrier. The bridge preserves it but does not extract or inject it. Its NATS side still performs outbound-only header injection. | Pass-through only plus outbound only |
| ACP remote HTTP and WebSocket | `acp-nats-server` initializes OpenTelemetry and wraps the official ACP HTTP server with `instrument_router`. The wrapper creates an HTTP server span but does not extract W3C HTTP headers. ACP models preserve `_meta`, but the bridge does not connect those keys to OpenTelemetry. WebSocket messages have the same ACP `_meta` requirement; the HTTP upgrade span is not a per-message trace parent. | Instrumented only plus pass-through only |
| ACP stdio | `acp-nats-stdio` initializes OpenTelemetry. ACP `_meta` is representable and preserved, but it is not extracted or injected. Selected Core NATS reply and notification paths inject current context, while direct JetStream prompt and request paths do not. | Pass-through only with partial outbound injection |
| Gateway HTTP webhooks | `trogon-gateway` initializes OpenTelemetry and `instrument_router` creates HTTP server spans. It does not extract incoming W3C headers. Source handlers construct JetStream headers for routing and deduplication, and `ClaimCheckPublisher` preserves those supplied headers, but neither layer injects the current OpenTelemetry context. | Instrumented only |
| Gateway WebSocket sources | Discord Gateway and Slack Socket Mode run inside the OTel-enabled gateway. No remote message context is extracted and their JetStream publishes do not receive injected trace headers. | Instrumented only |
| Decider event store | `CommandExecution::with_headers` can attach validated operational metadata to every emitted event. `trogon-decider-nats` persists and restores those values using the `Trogon-Header-` physical prefix. It does not capture the active OpenTelemetry context automatically. No non-test production call that supplies trace headers through `with_headers` was present at this review. | Carrier available |
| Scheduler event to execution publish | The scheduler extracts W3C fields from a consumed event's logical `Headers`, uses the extracted context as the processing span's parent, and reinjects it into the execution-schedule NATS publish. This works when the event already contains trace headers. The code does not add an ambient-context link, and the ordinary command path does not currently populate the event carrier. | Local handoff implemented; upstream feed missing |
| A2A HTTP, REST, and SSE | `a2a-nats-http` installs `tower-http`'s tracing layer but does not initialize `trogon-telemetry`, extract W3C headers, or inject context into its NATS requests. | Not OTel-enabled |
| A2A NATS, JetStream, stdio, bridge, and gateway | These paths do not install the repository OpenTelemetry runtime and do not use the shared NATS trace injection helpers. The gateway's `AuditTraceparent` is an optional audit data field, not an OpenTelemetry extractor or validated W3C carrier. | Not OTel-enabled |
| ARD HTTP registry | The router installs `tower-http`'s tracing layer, but the standalone surface does not install the repository OpenTelemetry runtime or a W3C extractor. | Not OTel-enabled |
| Browser Connect gateway | The accepted gateway ADR requires browser `traceparent` continuation onto NATS. No corresponding Connect gateway implementation was present in this checkout at this review. | Required by ADR; not implemented |
| OTLP exporter | OTel-enabled binaries export traces, metrics, and logs over HTTP through `trogon-telemetry`. | Export implemented; not a propagation hop |

## Where end-to-end correlation breaks

For an MCP or ACP call that begins in a host application, the intended flow is:

```text
host span
  -> protocol params._meta
  -> HTTP, WebSocket, or stdio bridge
  -> NATS message
  -> protocol server span
  -> downstream call
```

The current flow breaks because repository protocol senders do not inject the
active context into `params._meta`, receiving handlers do not extract it, NATS
consumers do not generally extract NATS headers, and the global propagator does
not include baggage. Outbound NATS injection can preserve an ambient span only
when one already exists in the task performing the send.

For durable events, the event-store adapter can persist the fields and the
scheduler can consume them, but the command boundary does not automatically
place the active context into event `Headers`. Persistence support therefore
does not yet produce a trace that survives the full command, event, scheduler,
and execution lifecycle.

## Current local conventions

- Keep MCP and ACP keys exactly `traceparent`, `tracestate`, and `baggage` under
  `params._meta`. Do not namespace them.
- Use the configured text-map propagator over direct NATS message headers. The
  exact `traceparent` name follows NATS ADR-41 as well as W3C Trace Context; do
  not invent a second set of `Trogon-*` names for the live carrier.
- Keep logical W3C names in durable event `Headers`. The event-store adapter's
  `Trogon-Header-` prefix is a physical storage mapping, not a new logical key.
- Keep a persisted event's trace metadata unchanged when reading, replaying,
  rebuilding from, or restoring that event. A span created by the operation is
  separate and is never written back into the historical event.
- Preserve the direct carrier through NATS server forwarding, JetStream
  redelivery, native source or mirror replication, server-side `RePublish`, and
  native snapshot restore. Treat an application consume-and-publish operation
  as a new message-creation boundary.
- Keep NATS broker path events distinct from application OpenTelemetry spans.
  Do not claim broker spans in an OpenTelemetry backend without an explicit
  path-event adapter.
- Treat correlation and causation identifiers as domain or workflow metadata,
  not as substitutes for OpenTelemetry trace context.
- Do not claim baggage propagation until a baggage propagator and its trust,
  filtering, and redaction policy are configured.
- Do not describe span export, `_meta` round-tripping, or a `TraceLayer` by
  itself as end-to-end propagation.

## Recommended resolutions

The current implementation matrix remains descriptive. The recommendations
below answer how each gap should be closed without collapsing protocol,
transport, and application ownership.

### MCP and ACP: instrument at the protocol boundary

The protocol role SDK or dispatcher should own `params._meta` instrumentation:

- When initiating a new request or notification, inject the active context into
  `params._meta` immediately before protocol encoding.
- When receiving a request or notification, extract `params._meta` immediately
  after protocol decoding and before creating the protocol server span or
  invoking the application callback.
- When forwarding the same logical message, preserve an existing valid protocol
  context instead of replacing it with the intermediary's transport context.
- Use the configured OpenTelemetry propagator for parsing and validation. Do not
  parse W3C values manually or make an invalid carrier a protocol error.

A gateway that terminates one protocol operation and initiates another creates
a new client span and injects that span's context. That is distinct from a
transport bridge carrying the same logical message, which preserves the
existing protocol context.

HTTP, WebSocket, stdio, and NATS adapters should carry the protocol message
unchanged and instrument only their own transport operation. When both a
protocol context and an ambient transport context exist, use the protocol
context as the remote parent of the protocol-processing span and link the
ambient transport context. This directly follows the OpenTelemetry MCP model;
ACP should use the same local model because its `_meta` carrier is likewise
transport-independent.

### NATS: link application context and preserve server-carried context

Generic NATS receive and process spans should link each extracted message
creation context rather than adopt it as their parent. This is the
OpenTelemetry messaging default and remains correct for batches, delayed
delivery, redelivery, and processing that already has an ambient span.

For a protocol carried over NATS, the protocol context in `_meta` remains the
parent of the protocol-processing span. The NATS header context describes the
transport hop and is a link. For a NATS message with no higher-level protocol
carrier, create a consumer process span in the current operation and link the
NATS message context. Outgoing work then injects the active process span, not
the historical linked context.

The scheduler's current parent-only behavior should move to this link model.
Using the event context as a parent may remain an explicit opt-in for a proven
single-message synchronous path, but it should not be the platform default.

NATS server operations do not create OpenTelemetry producer spans. Routes,
gateways, leaf nodes, JetStream storage and redelivery, native sources and
mirrors, server-side `RePublish`, and native snapshot restore should preserve
the direct message carrier they receive. Server-added `Nats-*` provenance and
path-tracing headers are separate from that application context.

An application that consumes and publishes creates a new NATS message. It
creates a producer or client span and injects that span's context into the new
direct carrier. A transparent protocol bridge still preserves the protocol
context in `_meta`; the new direct NATS context describes only its transport
hop.

[ADR#0042 (Draft)](../adr/0042-nats-trace-context-and-message-path-tracing.md)
proposes this link model, the compatible NATS version floor, and the operational
controls required before account `msg_trace` is enabled. Broker path events must
remain separate from application spans unless a later adapter defines their
OpenTelemetry mapping.

### Durable events: preserve creation context across storage lifecycles

The application boundary that starts `CommandExecution` should capture valid
`traceparent` and `tracestate` values as typed operational metadata and supply
them through `CommandExecution::with_headers`. The event-store adapter should
persist those values unchanged in the event envelope. It should not inspect the
ambient process or invent trace metadata during storage.

The storage mapping preserves one invariant:

```text
append:  logical traceparent=A
store:   physical Trogon-Header-traceparent=A
read:    logical traceparent=A
```

Reading the event does not replace `A` with the reader's current span context.
That remains true for normal reads, aggregate replay, projection rebuilds,
consumer recovery, and restoration from offloaded storage.
[OpenTelemetry messaging guidance](https://opentelemetry.io/docs/specs/semconv/messaging/messaging-spans/#context-propagation)
likewise treats a message creation context as producer-attached context that
intermediaries should preserve.

The lifecycle terms differ according to which layer, if any, creates something
new:

| Operation | Identity and stored context | Current operation context |
| --- | --- | --- |
| Read, replay, or rebuild | Reads the same event. Its logical `traceparent=A` remains unchanged. | The read or rebuild may run under span `B` and link to `A`. `B` is not written into the event. |
| Native JetStream snapshot restore | Restores the stored stream, messages, headers, timestamps, and stream state. Existing direct and event carriers remain `A`. | No per-message application publish occurs, so no per-message context `B` is injected. The restore operation can have its own operational span. |
| Native source, mirror, or server-side `RePublish` | Preserves the existing direct message carrier `A`. NATS adds server-owned source or republish provenance. | NATS does not create an application producer span or inject `B`. Carrier preservation alone does not promise that broker path-event collection continues across the operation. |
| Application restoration into another stream | Preserves the historical event's logical context `A`, but the application creates a new destination NATS message and stream position. | The restore job's producer span injects direct `traceparent=B`. The event-store encoding can simultaneously carry `Trogon-Header-traceparent=A`. [ADR#0013](../adr/0013-origin-stream-sequence-header.md) preserves origin sequence for this application-level workflow. |
| Import that creates a new domain event | Creates a new event rather than restoring the historical event. The new event receives its creation context `B`. | The import operation owns `B` and may link source context `A`. No accepted ADR reserves a second durable source-trace header. |
| Application consume-and-publish of an existing event | Preserves the embedded historical event context `A`, but creates a new NATS message envelope. | The direct NATS `traceparent=B` identifies the new application message creation context. The event-store encoding may simultaneously contain `Trogon-Header-traceparent=A`. |

If offloaded data returns with the same historical identity, the operation is a
restore, not an import. Native snapshot restore preserves the stored NATS
message. An application-level restore preserves the event identity and context
but creates a new outer NATS message context. If the import assigns a new event
identity, then both the event and message receive the new creation context. W3C
`traceparent` is propagation context, not a general-purpose provenance field.

For example, an application-level restore or consume-and-publish can carry both
contexts without changing the historical event. The same data has distinct
logical and physical views:

```text
direct NATS message header:
traceparent: B

logical event Headers:
traceparent: A
Trogon-Origin-Stream-Sequence: 120

physical event-store headers:
Trogon-Header-traceparent: A
Trogon-Header-Trogon-Origin-Stream-Sequence: 120
```

Here, `B` is the creation context of the new NATS message. `A` remains the
creation context stored in the historical event, and `120` records its original
stream position. A NATS server-side `RePublish` would instead preserve the
existing direct `traceparent=A` and add its documented `Nats-*` provenance
headers. It would not inject `B`.

If a newly created event needs durable source-trace provenance beyond a live
span link, that requires a separate metadata contract and an ADR. Do not invent
another trace header as part of an import implementation.

A live consumer should link the persisted event creation context to its process
span. If a distinct direct NATS message context also exists, it should link both
contexts without duplicating an identical link. A replay, rebuild, or repair
operation should keep its own operation span as the parent and link each
historical event context. It should never make an old producer span the parent
of the replay operation or synthesize new trace metadata for the historical
event.

This keeps historical provenance without making the duration, sampling choice,
or trust state of an old request control a later operational job.

### Baggage: preserve opaquely, activate by allowlist

Keep the global default trace-only. A bridge may preserve an existing
`baggage` field as opaque protocol data while it remains within the same declared
trust domain, but it should not extract, forward across a trust boundary, or
persist baggage by default.

Enabling baggage requires a typed boundary policy that allowlists keys, enforces
size limits, removes unknown entries, and forbids credentials, authorization
claims, and other secrets. Baggage must never be used as trusted input for
authentication, authorization, tenancy, or routing. This follows
[OpenTelemetry baggage security guidance](https://opentelemetry.io/docs/concepts/signals/baggage/),
which warns that baggage can reach unintended downstream resources and has no
built-in integrity protection.

### A2A: do not invent a protocol carrier

Use the configured W3C propagator over A2A HTTP headers and gRPC metadata, and
use the local NATS header carrier for the custom NATS binding. Internal
asynchronous task or event processing should use the message-link model above.
Do not add trace keys to A2A protocol metadata until A2A standardizes a carrier
or the repository accepts a specific extension.

### ADR scope

MCP `_meta` handling follows upstream guidance and can be implemented directly.
[ADR#0042 (Draft)](../adr/0042-nats-trace-context-and-message-path-tracing.md)
contains the focused local NATS decision: its official carrier, server
preservation boundary, asynchronous link model, durable replay relationship,
version floor, account controls, and relationship to broker path events. The ACP
parentage model and baggage trust policy remain recommendations on this page
until accepted by an ADR. They should not be silently folded into the NATS
carrier decision. A2A needs an ADR only if the repository later chooses a
custom protocol-level carrier.

The missing HTTP, MCP, ACP, and NATS extraction code that directly follows an
already accepted contract is an implementation gap, not a separate architecture
decision.

## Update rule

Update this page in the same change that adds or removes a propagator, changes a
carrier key, adds transport extraction or injection, persists trace metadata, or
changes which binaries initialize OpenTelemetry.

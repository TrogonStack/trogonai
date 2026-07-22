---
number: "0004"
slug: protocol-and-transport-layering
status: accepted
date: 2026-06-08
---

# ADR#0004: Protocol and Transport Layering

## Context

The repository will contain many [protocol](../glossary/protocol) role SDKs, transport adapters, [bridges](../glossary/bridge),
gateways, and application services. Without a strict layering rule, [ACP](../glossary/acp), [MCP](../glossary/mcp),
[NATS](../glossary/nats), HTTP, stdio, and callback SDK types can leak into every crate and make
domain code depend on whichever protocol or transport happened to arrive first.

This decision defines when protocol concepts are allowed to enter a component and
how transports should wrap or compose those components.

## Decision

Use this default layering:

```text
service/app/CLI binary
  -> transport adapter or listener
  -> protocol role SDK / protocol dispatcher
  -> application boundary
  -> domain crates and value objects
```

Protocols belong at protocol boundaries. Transports belong outside protocols.
Domain and application code should stay protocol-agnostic unless the protocol is
the product boundary being implemented.

## Protocol Injection Rules

Inject protocol types when the component exists to implement, expose, or adapt
that protocol:

- protocol model crates
- generated protocol crates
- protocol role SDKs
- protocol dispatchers
- protocol test harnesses
- bridges that translate one protocol transport to another
- gateways whose public product boundary is the protocol

Do not inject protocol types into domain code by default. Domain types, [deciders](../glossary/decider),
application services, and reusable workflow crates should use domain [commands](../glossary/command),
domain [events](../glossary/event), value objects, and ports. Convert protocol requests and responses
at the application boundary.

Protocol types may cross deeper only when that type is the explicit contract of
the component. Examples include a generated protocol crate, a protocol SDK, or a
persisted wire/event contract that has intentionally been selected as the durable
storage schema. When this happens, document the reason near the boundary and
convert back to validated domain types before ordinary business logic.

## Transport Layering Rules

Transports wrap protocol dispatch. They should not wrap arbitrary domain objects.

Use this shape:

```text
stdio / remote / NATS / HTTP listener
  -> frame and deliver protocol messages
  -> protocol dispatcher invokes callbacks
  -> callback implementation calls application/domain code
```

The transport owns framing, connection lifecycle, peer/session identifiers,
headers, timeouts, retries, backpressure, shutdown, and transport telemetry. It
does not own domain decisions.

The protocol dispatcher owns protocol roles, capabilities, callback routing,
request/response mapping, notifications, and protocol error semantics. It does
not own network listening or process I/O unless the package is intentionally a
combined protocol/transport adapter.

The application boundary owns the translation from protocol-level intent into
domain-level commands and queries.

## SDK Callback Rules

Protocol role SDKs are the right place to expose callback traits that use
protocol concepts. For example, an MCP server SDK can expose MCP server callback
traits, and an ACP agent SDK can expose ACP agent callback traits.

Callback implementers should be thin adapters. They may accept protocol request
types, but should convert them to domain inputs before invoking application
services. Do not pass NATS, stdio, HTTP, or WebSocket concepts into callback
traits unless the SDK is intentionally transport-specific.

Prefer transport-agnostic callback SDKs. Add transport-specific SDKs only when
the developer-facing callback model itself depends on that transport.

## Composition Root Rules

The service/app/CLI binary is the composition root. It wires together:

- runtime configuration
- telemetry identity
- transport listeners or bridges
- protocol role SDKs and dispatchers
- application callback implementations
- domain services and repositories

Runtime configuration decides which transports start. Cargo features may make
optional adapters available at compile time, but configuration decides what runs.

Combining multiple transports in one binary is valid only when the binary is one
operated workload, as defined in [ADR#0003](./0003-ai-protocol-transport-taxonomy.md).

## Package Boundary Rules

Keep protocol, transport, and domain package boundaries separate unless there is
a real reason to combine them.

Prefer:

```text
<protocol>                 protocol model or generated protocol types
<protocol>-<role>-sdk      callback SDK for one protocol role
<protocol>-<transport>     transport/backbone adapter
<protocol>-<transport>-<role>
```

Use a combined package only when the protocol and transport are not useful
independently, have one dependency surface, and have one implementer audience.

If a transport package starts to expose a developer callback API, consider
splitting a transport-agnostic role SDK out of it.

If a protocol SDK starts to own connection lifecycle, process I/O, HTTP routing,
or NATS subjects, move that behavior into a transport adapter.

## Examples

### ACP Agent over NATS

Preferred shape:

```text
acp-agent-sdk          callback traits and test harness for ACP agents
acp-nats              shared ACP-over-NATS routing model
acp-nats-agent        NATS adapter that connects ACP agent callbacks to NATS
```

The agent callback implementation may use ACP request/response types at the SDK
boundary. It should convert to domain inputs before calling business logic.

### MCP Server over Stdio and Remote HTTP

Preferred shape:

```text
mcp-server-sdk         callback traits for implementing MCP server behavior
mcp-nats              shared MCP-over-NATS routing model
mcp-nats-stdio        local stdio bridge
mcp-nats-remote       remote listener when multiple remote profiles are owned
```

Stdio and remote listeners can share protocol code. They should remain separate
when startup, shutdown, security posture, telemetry identity, or distribution
differs.

## Consequences

This keeps protocols and transports at the edge while still allowing protocol
SDKs to be ergonomic for developers implementing callbacks.

It also prevents transport names from becoming architecture. A domain capability
should not become "NATS-shaped" or "stdio-shaped" only because one transport is
currently used to reach it.

## References

- [ADR#0001: Workspace Runtime Taxonomy](./0001-workspace-runtime-taxonomy.md)
- [ADR#0002: Rust Crate Boundaries](./0002-rust-crate-boundaries.md)
- [ADR#0003: AI Protocol Transport Taxonomy](./0003-ai-protocol-transport-taxonomy.md)

---
number: "0003"
slug: ai-protocol-transport-taxonomy
status: accepted
date: 2026-06-08
---

# ADR#0003: AI Protocol Transport Taxonomy

## Context

AI protocols such as MCP and ACP separate protocol semantics from the transport
used to carry protocol messages. The same JSON-RPC lifecycle can run over a
local subprocess, a network endpoint, or an internal message bus bridge. Without
strict vocabulary, names such as `stdio`, `server`, `remote`, `http`,
`websocket`, `bridge`, and `nats` become interchangeable.

This decision defines how to name and reason about those pieces in this
repository.

## Decision

Use these meanings:

| Term | Meaning |
| --- | --- |
| Protocol | The data-layer contract: messages, lifecycle, capabilities, roles, and semantics. Examples: MCP, ACP. |
| Protocol role | One side of a protocol interaction, such as MCP client, MCP server, ACP client, or ACP agent. |
| Protocol role SDK | A developer-facing toolkit for implementing one protocol role's callbacks and lifecycle. |
| Transport | A binding that frames and delivers protocol messages. Examples: stdio, Streamable HTTP, WebSocket. |
| Local transport | A transport where one process launches or directly supervises the other on the same machine. |
| Remote transport | A transport exposed over a network boundary. |
| Connectivity profile | A concrete remote connection shape on the same endpoint, such as Streamable HTTP or WebSocket. |
| Bridge | A process or library that translates between transports while preserving the same protocol semantics. |
| Gateway | A production edge component that accepts external traffic and routes it inward. A gateway may contain bridges, but the name implies operational edge ownership. |
| Backbone | An internal routing substrate such as NATS. It is not automatically the public protocol transport. |
| API style | The shape of an externally exposed API, such as ConnectRPC, gRPC, REST-like HTTP, or GraphQL. |
| API description | A machine-readable contract for an API surface, such as an OpenAPI document. |

Protocol names and transport names should not be mixed. For example, ACP is the
protocol, stdio is a local transport, Streamable HTTP and WebSocket are remote
connectivity profiles, and NATS is the internal backbone used by this repository
to route protocol messages.

Protocol role names must also stay distinct from runtime placement. An MCP
server SDK is a toolkit for implementing the MCP server role. It is not
automatically a deployable service, a remote listener, or a package that belongs
under `services/`.

## Boundary Selection Order

Choose the narrowest boundary that satisfies the integration:

1. Prefer the internal NATS backbone when both sides are first-party runtime
   components and the interaction can use the repository's internal routing,
   queueing, or eventing model.
2. Use a first-party service API only when a caller needs an explicit API surface,
   generated client/server contract, browser-compatible HTTP access, network edge
   exposure, or compatibility boundary that should not depend on NATS.
3. For first-party service APIs, prefer ConnectRPC over direct gRPC when Protocol
   Buffers and generated clients are viable.
4. Use direct gRPC when ConnectRPC cannot satisfy the integration requirement or
   when a caller/runtime specifically requires native gRPC semantics.

Do not introduce ConnectRPC, gRPC, GraphQL, OpenAPI, or REST-like HTTP only
because two internal components need to communicate. If the boundary is internal
and can live on the backbone, model it as a NATS-backed protocol, command, event,
or queue contract instead.

## Standard Transport Categories

### Stdio

`stdio` means local subprocess transport over standard input and standard output.
Use it when an AI host, IDE, CLI, or local client launches a companion process
and exchanges newline-delimited JSON-RPC over `stdin` and `stdout`.

Use the `-stdio` suffix only for packages or binaries whose public boundary is a
local stdio surface. A stdio package may be a bridge if it forwards the protocol
to an internal backbone.

### Streamable HTTP

`streamable-http` means a remote HTTP transport profile. Use this term when the
public boundary is HTTP request/response plus streaming behavior.

For MCP, Streamable HTTP is a standard transport. For ACP, the current remote
transport draft defines Streamable HTTP with POST/GET/DELETE and long-lived SSE
streams.

Use `-streamable-http` when the package specifically owns that HTTP transport.
Use `-http` only when the package is intentionally broader than Streamable HTTP.

### WebSocket

`websocket` means a persistent, bidirectional remote connection profile. Use it
when the package specifically owns WebSocket framing or upgrade behavior.

Do not name a package `-websocket` if WebSocket is only one profile inside a
larger remote listener that also owns Streamable HTTP on the same endpoint.

### Remote

`remote` means a network-facing transport surface that may support more than one
remote connectivity profile. Use it when one runtime owns the remote endpoint
and composes multiple profiles together, such as Streamable HTTP plus WebSocket.

Prefer `-remote` over `-server` when the boundary is "remote transport listener"
rather than a protocol server role.

### NATS

`nats` means the internal backbone or adapter used to route protocol messages
inside the system. It is not the same thing as stdio, Streamable HTTP, or
WebSocket.

Use `-nats` for packages whose public boundary is the protocol-to-NATS adapter or
shared NATS routing model.

## API Styles and Descriptions

Not every API-related word is a transport:

- `http` is a broad network protocol and API delivery substrate.
- `streamable-http` is a specific streaming HTTP transport profile.
- `websocket` is a persistent bidirectional connection profile.
- `connectrpc` is the preferred first-party RPC API style when the integration
  can use Protocol Buffers and generated clients.
- `grpc` is an RPC framework and API style that commonly uses Protocol Buffers
  and HTTP/2. Use it when direct gRPC compatibility is required or ConnectRPC is
  not viable.
- `graphql` is an API query language and server runtime model.
- `openapi` is an API description format for HTTP or HTTP-like APIs, not a
  runtime transport. OpenAPI-first APIs are disallowed by default.

Use package names for the boundary being owned, not for every label involved in
the stack. A single remote API service may expose REST-like HTTP endpoints,
OpenAPI documentation, WebSocket subscriptions, and GraphQL queries without
requiring one package per label.

After a first-party service API surface is necessary, prefer ConnectRPC whenever
possible. ConnectRPC is the API default because it gives the repository a typed
Protocol Buffer contract, generated clients, browser-compatible HTTP APIs, and
gRPC compatibility from the same service definition.

Use direct gRPC when ConnectRPC cannot satisfy the integration requirement or
when a caller/runtime specifically requires native gRPC semantics.

Use OpenAPI as the primary API contract only when ConnectRPC or gRPC cannot be
used. Acceptable reasons include third-party ecosystem requirements, existing
HTTP API compatibility, platform constraints, webhook-style integrations, or
external clients that require OpenAPI/REST-like HTTP. The reason must be stated
near the API boundary. Do not choose OpenAPI only because it is familiar.

Create a package per API style only when the style owns a distinct boundary:

- different generated client/server code
- different dependency set
- different schema or compatibility contract
- different runtime model or connection lifecycle
- different security posture
- different telemetry identity or operational ownership
- independent dependents that should import only that API surface

Keep API styles in one package when they are one cohesive external API surface
with shared routing, auth, config, deployment, telemetry identity, release
cadence, and ownership.

Treat OpenAPI documents as exception contracts for HTTP APIs. Keep the OpenAPI
document with the HTTP API surface it describes unless generated OpenAPI
client/server code becomes a reusable dependency boundary. In that case, a
dedicated `-openapi` or `-client` package may be appropriate, but the exception
still needs a reason.

## Naming Rules

Prefer names that describe the actual public boundary:

```text
<protocol>-<backbone>             shared protocol/backbone adapter
<protocol>-<backbone>-stdio       local stdio bridge to the backbone
<protocol>-<backbone>-remote      remote listener backed by the backbone
<protocol>-<backbone>-streamable-http
<protocol>-<backbone>-websocket
```

Use `-remote` when one package intentionally owns multiple remote connectivity
profiles. Use the specific transport suffix when one package owns exactly that
transport.

Use `-connectrpc`, `-grpc`, `-graphql`, or `-openapi` only when that API style or
generated contract is the owned package boundary. Do not add those suffixes only
because a runtime happens to expose that style as one route group.

Avoid `-server` unless the package's public role is clearly a protocol server or
a network server in the conventional sense. Do not use `server` as a generic
synonym for production runtime, remote transport, or deployable workload.

## Package Boundary Rules

Do not create a separate package for every transport name up front.

Create a separate package when the transport has a distinct dependency set,
runtime model, configuration surface, telemetry identity, security posture,
operational ownership, or independent dependents.

The same rule applies to API styles. Do not create one package per HTTP,
Streamable HTTP, WebSocket, ConnectRPC, gRPC, GraphQL, or OpenAPI label unless
that label corresponds to a real dependency, runtime, compatibility,
generated-code, or ownership boundary.

Keep multiple transport profiles in one package when they are one cohesive
runtime surface. Streamable HTTP and WebSocket often belong together when they
share endpoint routing, connection/session management, auth, telemetry identity,
deployment, and ownership.

Keep stdio separate when it is a local bridge with different startup, shutdown,
I/O, security, telemetry, or distribution expectations from the remote listener.

Use runtime configuration, not Cargo features, to choose which transports a
single runtime starts. Use Cargo features only for additive optional transport
capabilities that can be enabled together.

## Combined Binaries

A binary is a runtime composition boundary. Combining multiple transports or API
styles into one binary is valid when the result is intentionally one operated
workload.

Use one binary when the combined runtime has:

- one deployment unit
- one process lifecycle
- one configuration surface
- one OpenTelemetry service identity
- one security boundary
- one release cadence
- one operational owner
- shared scaling and failure characteristics

Do not combine into one binary when the parts need independent deployment,
scaling, failure isolation, security posture, telemetry identity, release
cadence, or ownership.

Combining binaries does not require collapsing library boundaries. Prefer a thin
binary that composes reusable protocol, transport, client, and adapter crates:

```text
combined-runtime binary
  -> protocol/backbone crate
  -> transport adapter crates
  -> API style crates or generated client/server crates
```

Use typed runtime configuration to decide which listeners or bridges start. Do
not model runtime startup choices as Cargo features. Cargo features may make
optional adapters available at compile time, but configuration decides which
adapters run.

When one binary starts multiple externally visible surfaces, name it by the
owned runtime boundary rather than by every transport it exposes. For example,
use a product or workload name when HTTP, WebSocket, GraphQL, and OpenAPI are
all part of the same API workload.

## Current Repository Mapping

Current names should be interpreted this way:

- `acp-nats`: shared ACP-over-NATS adapter and routing boundary.
- `acp-nats-stdio`: local ACP stdio bridge backed by NATS.
- `acp-nats-server`: current remote ACP transport listener backed by NATS. A
  clearer future name would be `acp-nats-remote` if it continues to own both
  Streamable HTTP and WebSocket.
- `mcp-nats`: shared MCP-over-NATS adapter and routing boundary.
- `mcp-nats-stdio`: local MCP stdio bridge backed by NATS.
- `mcp-nats-server`: current remote MCP Streamable HTTP listener backed by NATS.
  A clearer future name would be `mcp-nats-streamable-http` if it only owns
  Streamable HTTP, or `mcp-nats-remote` if it owns multiple remote profiles.

Renaming existing packages is not part of this decision. This ADR defines the
vocabulary for future naming and future migrations.

## Consequences

The repository should distinguish:

- Protocol role: MCP server, MCP client, ACP agent, ACP client.
- Transport binding: stdio, Streamable HTTP, WebSocket.
- Runtime placement: local subprocess, remote listener, production workload.
- Internal routing: NATS backbone.

This prevents `server` from becoming a catch-all word and keeps stdio-vs-remote
decisions grounded in runtime behavior instead of package-count preference.

## References

- [MCP transport overview](https://modelcontextprotocol.io/specification/draft/basic/transports)
- [MCP architecture overview](https://modelcontextprotocol.io/docs/learn/architecture)
- [ACP Streamable HTTP and WebSocket transport RFD](https://agentclientprotocol.com/rfds/streamable-http-websocket-transport)
- [ConnectRPC introduction](https://connectrpc.com/docs/introduction/)
- [ConnectRPC protocol reference](https://connectrpc.com/docs/protocol/)
- [gRPC introduction](https://grpc.io/docs/what-is-grpc/introduction/)
- [GraphQL overview](https://graphql.org/)
- [OpenAPI introduction](https://learn.openapis.org/introduction.html)

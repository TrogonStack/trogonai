# Agent protocol support

This reference describes the protocol adapters in the repository, reviewed on
2026-11-16. An implemented adapter is source-level support, not a claim that a
deployment has passed interoperability, load or production acceptance.

For the role of each protocol, see [How agent protocols fit together](../architecture/agent-protocols.md).

## Transport surfaces

| Protocol | Shared NATS adapter | Local surface | Remote surface |
| --- | --- | --- | --- |
| A2A | `a2a-nats` client and server | `a2a-nats-stdio` | `a2a-nats-http` for local agents behind NATS; `a2a-bridge` for the separate HTTPS cross-binding edge |
| ACP | `acp-nats` and its bridge traits | `acp-nats-stdio` | `acp-nats-server`, using the official SDK HTTP and WebSocket transport |
| MCP | `mcp-nats`, implementing the official Rust SDK transport traits for both roles | `mcp-nats-stdio` | `mcp-nats-server`, serving Streamable HTTP on `/mcp` |

NATS subjects and bridge configuration are repository-specific bindings. Their
existence does not mean upstream protocol clients connect directly to NATS.

## A2A

### Implemented surfaces

- Message send and streaming, task get/list/cancel/resubscribe, push notification
  configuration and agent-card retrieval have typed method and subject mappings.
- Task events use JetStream-backed paths. The client and server adapters carry
  the protocol between callers and agent implementations.
- The HTTP adapter exposes JSON-RPC, streaming, REST routes and agent-card
  discovery at `/.well-known/agent-card.json`.
- The gateway owns layered policy enforcement and audit on its ingress path.
  The separate `a2a-bridge` HTTP edge verifies AAuth proof of possession and
  obtains caller-scoped NATS credentials through the auth callout.

### Boundaries

The HTTP adapter, HTTPS bridge and gateway are different components. Their
security properties depend on the configured path, caller credentials and
broker permissions. Do not infer authenticated public ingress from the presence
of an HTTP listener alone.

The adapters do not implement an agent's business behavior or the platform's
definition/revision registry. Task transport is distinct from the proposed
[workflow execution model](../architecture/workflows.md).

### Source and decisions

- [`a2a-nats` method mappings](https://github.com/TrogonStack/trogonai/blob/main/rsworkspace/crates/a2a/a2a-nats/src/a2a_method.rs)
- [`a2a-nats-http` router](https://github.com/TrogonStack/trogonai/blob/main/rsworkspace/crates/a2a/a2a-nats-http/src/router.rs)
- [`a2a-gateway` runtime](https://github.com/TrogonStack/trogonai/tree/main/rsworkspace/crates/a2a/a2a-gateway/src/runtime)
- [`a2a-bridge` edge authentication](https://github.com/TrogonStack/trogonai/tree/main/rsworkspace/crates/a2a/a2a-bridge/src/edge_auth)
- [ADR#0082: bus principal and edge proof of possession](../adr/0082-bus-principal-from-subject-and-edge-proof-of-possession.md)

## ACP

### Implemented surfaces

The bridge routes initialization and authentication, session creation/loading,
prompts and cancellation, session management, provider operations and supported
extensions. Client callbacks include file access, permission requests, session
updates, terminal operations and session-scoped elicitation.

The remote listener uses the official SDK's HTTP and WebSocket transport, with
compatibility layers for Origin enforcement, protocol-version validation and
SSE buffering behavior.

The [ACP conformance matrix](../architecture/acp-conformance.md) is the authority
for exact methods, payload variants, dependency pins and observable remote
transport differences. Consult it when upgrading the SDK or comparing an
upstream client with this bridge.

### Known gaps

- Request-scoped native MCP messages (`mcp/message`) are not routed. Preserving
  MCP capability payloads does not implement their lifecycle.
- Connection-scoped `auth/status_update` push notifications and pre-session
  elicitation do not have a destination in the session-scoped client routing.
- Proxy-chain initialization and successor handoff are not supported by this
  agent endpoint.
- HTTP and WebSocket delivery do not establish full conformance with the
  evolving remote transport RFD. The matrix records the adopted SDK's behavior
  and [remote transport differences](../architecture/acp-conformance.md#remote-transport-behavior-changes).
- Remote caller authentication and caller-bound subjects remain draft work
  under [ADR#0087](../adr/0087-mcp-acp-caller-identity-from-subject-and-edge-credential.md).

### Source and operations

- [`acp-nats` bridge](https://github.com/TrogonStack/trogonai/tree/main/rsworkspace/crates/acp/acp-nats/src)
- [`acp-nats-server` composition](https://github.com/TrogonStack/trogonai/blob/main/rsworkspace/crates/acp/acp-nats-server/src/main.rs)
- [ACP conformance](../architecture/acp-conformance.md)
- [Retire the ACP notifications stream](../how-to/retire-acp-notifications-stream.md)
- [ADR#0020: ACP SDK boundary and bridge traits](../adr/0020-acp-sdk-1x-boundary-and-bridge-traits.md)

## MCP

### Implemented surfaces

- The shared NATS adapter implements transport connections for MCP client and
  server roles using the official Rust SDK, `rmcp`.
- The HTTP proxy forwards tools, resources, resource subscriptions, prompts,
  completions, discovery, supported task operations and custom requests.
  Server requests and notifications can travel back to the HTTP client.
- MCP NATS bodies preserve canonical JSON-RPC messages. Selected MCP routing
  headers and request metadata survive the HTTP proxy; arbitrary HTTP headers
  and caller credentials are not forwarded as protocol metadata.
- The stdio bridge and Streamable HTTP listener connect clients to a selected
  NATS-backed MCP server. The backend determines available capabilities.

### Boundaries

`mcp-nats` is a protocol transport. It does not provide a managed tool registry,
per-tool policy enforcement or third-party OAuth custody. Ordinary NATS
subscriptions do not imply NATS micro discovery or queue-balanced service
behavior.

The Streamable HTTP listener's allowed-host check is a DNS-rebinding defense,
not caller authentication. It uses a shared NATS connection with generated
client peer IDs. The verified caller boundary remains proposed in
[ADR#0087](../adr/0087-mcp-acp-caller-identity-from-subject-and-edge-credential.md).

The [connector MCP authorization reference](./connector-mcp-authorization.md)
describes a separate outbound credential-custody contract. That design is not
evidence that inbound MCP authorization or the managed connector service is
deployed.

### Source and decisions

- [`mcp-nats` transport](https://github.com/TrogonStack/trogonai/blob/main/rsworkspace/crates/mcp/mcp-nats/src/transport.rs)
- [`mcp-nats` wire codec](https://github.com/TrogonStack/trogonai/blob/main/rsworkspace/crates/mcp/mcp-nats/src/wire.rs)
- [`mcp-nats-server` proxy](https://github.com/TrogonStack/trogonai/blob/main/rsworkspace/crates/mcp/mcp-nats-server/src/runtime.rs)
- [ADR#0041: canonical MCP JSON-RPC bodies](../adr/0041-canonical-mcp-jsonrpc-bodies-over-nats.md), draft
- [ADR#0054: NATS protocol binding documentation](../adr/0054-nats-protocol-binding-documentation.md), draft

## Upstream protocols

- [A2A documentation](https://a2a-protocol.org/)
- [Agent Client Protocol documentation](https://agentclientprotocol.com/)
- [Model Context Protocol documentation](https://modelcontextprotocol.io/)

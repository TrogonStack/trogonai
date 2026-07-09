---
number: "0020"
slug: acp-sdk-1x-boundary-and-bridge-traits
status: accepted
date: 2026-07-07
---

# ADR 0020: ACP SDK 1.x Boundary and Bridge-Owned Callback Traits

## Context

The `agent-client-protocol` Rust SDK was redesigned in its 0.11.0 release and
stabilized as 1.x. Two changes force a decision here:

1. The SDK's `Agent` and `Client` traits no longer exist. They are role marker
   structs now; message handling is registered through builder callbacks
   (`Agent.builder().on_receive_request(...)`) and outbound calls go through
   `ConnectionTo<Role>::send_request(...)`. Our crates used those traits as the
   internal contract everywhere: runners implement `Agent`, the server-side
   `Bridge` implements `Agent`, and `NatsClientProxy` implements `Client`
   ([ADR 0004](./0004-protocol-and-transport-layering.md) describes this
   layering).
2. The SDK now ships official transports (`ByteStreams`, and HTTP/WebSocket in
   `agent-client-protocol-http`). Our ACP-over-NATS transport is hand-rolled,
   which raises the question of whether the official transport abstraction
   should replace parts of it.

## Decision

### 1. Keep the hand-rolled NATS transport

The SDK transports model point-to-point byte streams between exactly two
peers. The NATS leg is not that: it is subject-routed (per-session subjects,
global subjects, wildcard subscriptions), durable where required (JetStream
COMMANDS stream with keepalive acks), and multi-peer. Flattening it into a
`ByteStreams` pair would discard the routing model that ADR 0003 and ADR 0004
establish. The SDK builder connections are used only at true byte-stream
boundaries: the WebSocket duplex in `acp-nats-server` and stdio in
`acp-nats-stdio`.

### 2. Bridge-owned callback traits replace the removed SDK traits

`acp-nats` defines its own `AgentHandler` and `ClientHandler` traits,
mirroring the method surface the bridge routes (the SDK's `schema::v1::*`
request/response types remain the argument and return types, so wire
compatibility is unchanged). The names avoid colliding with the 1.x SDK's own
`AcpAgent` subprocess helper. Runners implement `AgentHandler`; the
server-side `Bridge` implements `AgentHandler` by forwarding over NATS;
`NatsClientProxy` implements `ClientHandler`. This was
already the intended shape in ADR 0004 (an "ACP agent SDK" exposing agent
callback traits); the SDK redesign makes it mandatory rather than optional.

### 3. SDK builder callbacks adapt boundaries to the bridge traits

At each byte-stream boundary, a thin adapter registers one `on_receive_*`
callback per routed method and delegates to the `AgentHandler`/`ClientHandler`
implementation. Outbound calls from the bridge to the peer go through the
connection handle (`ConnectionTo<...>`). Adapters contain no logic beyond
delegation, per the zero-cost passthrough rule in `rsworkspace/crates/AGENTS.md`.

The adapters are shared by `acp-nats-server` (WebSocket and HTTP duplex) and
`acp-nats-stdio`, so they live in one place: the `boundary` module of
`acp-nats`. That module is the single SDK-connection-aware part of the crate;
the NATS routing core remains free of connection machinery.

## Consequences

- The bridge's method surface is defined in one place (the bridge traits), and
  the conformance matrix (`docs/architecture/acp-conformance.md`) tracks it.
- New spec methods require touching trait, adapter, and subject mapping. The
  upgrade ritual in the conformance doc makes that explicit instead of
  accidental.
- The SDK's own request cancellation, session helpers, and future transport
  work apply at the boundaries without constraining the NATS leg.
- We keep full control of JetStream durability semantics, backpressure, and
  keepalives, which the SDK transport abstraction does not model.

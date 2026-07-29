---
number: "0020"
slug: acp-sdk-1x-boundary-and-bridge-traits
status: accepted
date: 2026-07-07
---

# ADR#0020: ACP SDK 1.x Boundary and Bridge-Owned Callback Traits

## Context

The `agent-client-protocol` Rust SDK was redesigned in its 0.11.0 release and
stabilized as 1.x. Two changes force a decision here:

1. The SDK's `Agent` and `Client` traits no longer exist. They are role marker
   structs now; message handling is registered through builder callbacks
   (`Agent.builder().on_receive_request(...)`) and outbound calls go through
   `ConnectionTo<Role>::send_request(...)`. Our crates used those traits as the
   internal contract everywhere: runners implement `Agent`, the server-side
   `Bridge` implements `Agent`, and `NatsClientProxy` implements `Client`
   ([ADR#0004](./0004-protocol-and-transport-layering.md) describes this
   layering).
2. The SDK now ships official [transports](../glossary/transport) (`ByteStreams`, and HTTP/WebSocket in
   `agent-client-protocol-http`). Our [ACP](../glossary/acp)-over-[NATS](../glossary/nats) transport is hand-rolled,
   which raises the question of whether the official transport abstraction
   should replace parts of it.

## Decision

### 1. Keep the hand-rolled NATS transport

The SDK transports model point-to-point byte streams between exactly two
peers. The NATS leg is not that: it is subject-routed (per-session subjects,
global subjects, wildcard subscriptions), durable where required ([JetStream](../glossary/jetstream)
COMMANDS stream with keepalive acks), and multi-peer. Flattening it into a
`ByteStreams` pair would discard the routing model that
[ADR#0003](./0003-ai-protocol-transport-taxonomy.md) and
[ADR#0004](./0004-protocol-and-transport-layering.md) establish. The SDK builder connections are used only at true byte-stream
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
already the intended shape in
[ADR#0004](./0004-protocol-and-transport-layering.md) (an "ACP agent SDK" exposing agent
callback traits); the SDK redesign makes it mandatory rather than optional.

### 3. SDK builder callbacks adapt boundaries to the bridge traits

At each byte-stream boundary, a thin adapter registers one `on_receive_*`
callback per routed method and delegates to the `AgentHandler`/`ClientHandler`
implementation. Outbound calls from the bridge to the peer go through the
connection handle (`ConnectionTo<...>`). Adapters contain no logic beyond
delegation, per the zero-cost passthrough rule in `rsworkspace/crates/AGENTS.md`.

> Amended 2026-07-09: the inbound adapter is now a single
> `on_receive_dispatch` handler typed over the SDK's
> `ClientRequest`/`ClientNotification` enums, whose method table the SDK
> already maintains, instead of one registration per method. Delegation
> semantics are unchanged; the per-method surface lives only in the bridge
> trait and one match expression.

> Amended 2026-07-27: this decision survives the SDK 2.0 major bump unchanged.
> 2.0 rewrites the transport boundary around batch-aware frames and renames the
> response-router methods, but because the adapter delegates through the
> `ClientRequest`/`ClientNotification` enums rather than the low-level channel,
> the entire migration was one method rename. Inbound JSON-RPC batches now work
> through the bridge for free: the SDK splits them into independent dispatches
> and regroups the replies, so the NATS leg keeps carrying one message per
> subject message.

The adapters are shared by `acp-nats-server` (WebSocket and HTTP duplex) and
`acp-nats-stdio`, so they live in one place: the `boundary` module of
`acp-nats`. That module is the single SDK-connection-aware part of the crate;
the NATS routing core remains free of connection machinery.

### 4. Scope correction: the HTTP/WebSocket boundary was never evaluated

> Added 2026-07-27.

Decision 1 above asks one question, "should the official transports replace the
NATS transport", and answers it correctly: no. But `agent-client-protocol-http`
is named in the Context and then never assessed on its own terms, and decision 1
already concedes that the WebSocket duplex in `acp-nats-server` and stdio in
`acp-nats-stdio` *are* true byte-stream boundaries. Those are precisely the
boundaries that crate serves. The question that went unasked is therefore
"should the official HTTP/WebSocket transport serve our byte-stream boundary",
and nothing in the original reasoning answers it.

The consequence is measurable. `acp-nats-server/src/transport.rs` hand-rolls the
draft remote transport in 1,507 lines against an official crate that uses the
same header names (`acp-connection-id`, `acp-session-id`), the same
initialize-creates-the-connection contract, the same session-scoped method list,
and the same web framework. Reuse there would delete most of that file.

This ADR is not reversed, because the reuse is not currently a good trade:

- Four behaviors we depend on have no equivalent in the official crate, and
  three of them are places where we are *ahead* of it, not merely different:
  `Acp-Protocol-Version` validation (the transport spec says clients SHOULD send
  it and upstream does not read it at all), server-side `Origin` enforcement on
  every verb rather than only the WebSocket upgrade, and a graceful drain hook.
  The fourth, UUIDv7 connection ids, is a preference.
- `ConnectionRegistry` is entirely `pub(crate)`, so none of the four can be
  layered on from outside `into_router()`. Closing them means upstream PRs or a
  fork.
- At 2.0.0 the crate has no other dependents on crates.io. Trading tested code
  for an unexercised major version is the wrong direction today, and gets less
  wrong every release.

What changes now is only that the gap is tracked instead of invisible: the
`## Companion crates` table in `docs/architecture/acp-conformance.md` records
every companion crate and why it is or is not adopted, and the freshness task
reads that table so a companion release reaches us the same way a core release
does. Revisit when the four gaps close or when adoption elsewhere demonstrates
the crate is exercised.

### 5. The HTTP/WebSocket boundary now uses the official transport

> Added 2026-07-28. Supersedes the "not currently a good trade" conclusion in
> decision 4, which was argued from a blocker that turned out not to exist.

Decision 4 read the `Send` bound on `ConnectTo` as a structural obstacle. It is
not: every `ClientHandler` implementation was already `Send + Sync`, and the
`!Send`-ness came from one `Rc`, one `Cell`, and two `spawn_local` calls, all
incidental. Converting them was mechanical, removed a scalability ceiling where
all client-side work for every connection shared one thread, and retired the
`LocalSet` from `acp-nats-stdio` entirely.

With that gone, `acp-nats-server` serves `AcpHttpServer` over a per-connection
`NatsAgentComponent`, and `transport.rs` plus `connection.rs` are deleted: 3,052
lines including their tests. `compat.rs` layers back the three behaviors upstream
omits, and both `main` and the tests build the router through the same function
so the served and tested stacks cannot drift.

Decision 1 stands unchanged and is the point: the NATS leg is still hand-rolled,
because its wire format carries no JSON-RPC envelope at all (method identity is
the subject token, the id is a header, the body is bare params) and no
byte-stream transport can express that. What moved upstream is only the part that
was always a byte-stream boundary.

Two costs were accepted rather than solved. Connection ids are now UUIDv4 because
`ConnectionRegistry::next_connection_id` is private, so `AcpConnectionId` no
longer governs the wire and was deleted. And a browser WebSocket upgrade is
rejected, because `ServerOptions::default` refuses any request carrying an
`Origin` header on that path; non-browser clients are unaffected. Both are
recorded, with the rest of the observable differences, under "Remote transport
behavior changes" in the conformance doc.

## Consequences

- The bridge's method surface is defined in one place (the bridge traits), and
  the conformance matrix (`docs/architecture/acp-conformance.md`) tracks it.
- New spec methods require touching trait, adapter, and subject mapping. The
  upgrade ritual in the conformance doc makes that explicit instead of
  accidental. (Amended 2026-07-09: the adapter cost is one match arm in the
  boundary dispatch rather than a shim function plus a registration; the
  ritual itself is unchanged.)
- The SDK's own request cancellation, session helpers, and future transport
  work apply at the boundaries without constraining the NATS leg.
- We keep full control of JetStream durability semantics, backpressure, and
  keepalives, which the SDK transport abstraction does not model.
- The remote transport is upstream's as of decision 5, so HTTP, WebSocket, and
  stdio all share one framing implementation and batch handling lives in exactly
  one place. The NATS leg remains ours for the reason decision 1 gives.
- Companion crates are tracked in the conformance doc and watched by the
  freshness task, so an upstream release re-tests these decisions instead of
  letting them drift.

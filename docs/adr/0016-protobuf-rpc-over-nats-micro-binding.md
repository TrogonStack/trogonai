---
number: "0016"
slug: protobuf-rpc-over-nats-micro-binding
status: accepted
date: 2026-07-04
---

# ADR 0016: Protocol Buffers RPC over NATS micro Binding

## Context

[ADR 0009](./0009-protocol-buffers-wire-contracts.md) makes Protocol Buffers the
wire contract for first-party owned service and message contracts.
[ADR 0011](./0011-jsonrpc-over-nats-binding.md) defines how the JSON-RPC family
(ACP, MCP, A2A) is carried over the NATS backbone, but it is explicitly scoped to
JSON-RPC. There is no equivalent rule for how a first-party **protobuf** service
is carried over NATS, so any such service would have to invent its own subject
scheme, success/error signaling, and discovery. ADR 0011 exists because three
JSON-RPC mappings diverged and lost errors; a protobuf service on NATS is exposed
to the same class of drift with no governing rule.

`trogon-proto` already ships
[`trogon.nats.micro.v1alpha1`](https://github.com/TrogonStack/trogon-proto/blob/main/proto/trogon/nats/micro/v1alpha1/options.proto),
a protocol-neutral set of options that attach NATS Services metadata to any
protobuf `service` (`ServiceOptions`: `version`, `description`, `metadata`,
`content_type`) and `rpc` method (`MethodOptions`: endpoint `metadata`). Its own
example is a generic `OrderService`. This shared mechanism has no ADR governing
what it means on the wire.

NATS Services (NATS micro,
[ADR-32](https://github.com/nats-io/nats-architecture-and-design/blob/main/adr/ADR-32.md))
already provides the RPC substrate a protobuf service needs: request/reply,
queue-group load balancing, discovery (`$SRV.PING|INFO|STATS`), versioning,
per-endpoint stats, and a standard error channel (`Nats-Service-Error`,
`Nats-Service-Error-Code`). This ADR binds annotated protobuf services to that
substrate as one shared rule, the protobuf-family sibling of ADR 0011.

This is a naming binding, not a protocol tunnel. It borrows gRPC's *idiom*
(method-in-path routing, canonical status semantics) so the shape is familiar. It
does not run gRPC: there is no HTTP/2, no gRPC framing, and no gRPC library on the
path. The transport is core NATS; the RPC semantics are NATS micro.

## Decision

### 1. An annotated protobuf service is a NATS micro service

A protobuf `service` carrying `option (trogon.nats.micro.v1alpha1.service)` is
registered as one NATS micro service. Each `rpc` becomes a micro **endpoint**.
`ServiceOptions.version`, `description`, and `metadata` populate the service's
discovery record; `MethodOptions.metadata` populates the endpoint's. The proto
`service` is the canonical wire contract, and the registered micro service must
expose exactly its methods as endpoints.

### 2. The subject is derived from the service and the method

An endpoint's subject is `<group>.<EndpointName>`, where the group is a configured
prefix plus the service name and the endpoint name is the `rpc` method name.

| Field | NATS location | Rule |
| --- | --- | --- |
| Service | subject group | `<subject_prefix>.<service-name>` |
| Method | endpoint subject suffix | The `rpc` method name. Mirrors gRPC `/package.Service/Method` |
| Request | request payload | The request message bytes |
| Reply | reply payload | The response message bytes |

The request **message type** is a property of the endpoint (held from the service
descriptor), never recovered by parsing the subject. Because micro subscribes only
to declared endpoints, routing is structural: an unknown method has no
subscription and is unroutable at the client.

### 3. Success and error discriminate on the micro error header

A reply is an error if, and only if, `Nats-Service-Error-Code` is present. On
success the reply body is the response message; on a service error the body may be
empty and the failure is carried by `Nats-Service-Error` (message) and
`Nats-Service-Error-Code` (numeric). Middleware routes on the subject and meters on
the error header without decoding the body, exactly as ADR 0011 requires for
`Jsonrpc-Error-Code`.

The micro error channel signals **service faults**: malformed input, timeouts,
and handler failures, which micro also counts in `num_errors`. A defined
*application-level negative outcome that still executed successfully* (for example
a validation result the caller acts on) belongs in the typed response body, not on
the error channel, so that `num_errors` stays a health signal rather than a
business-outcome counter. Each specializing ADR defines its own outcome taxonomy
over this rule.

### 4. Content type is negotiated from the service option

`ServiceOptions.content_type` governs the request and reply `Content-Type` header.
`CONTENT_TYPE_UNSPECIFIED` accepts both `application/protobuf` and
`application/json` on the same endpoints; `CONTENT_TYPE_PROTOBUF` or
`CONTENT_TYPE_JSON` restrict it. JSON is `google.protobuf.Any`-style canonical
JSON handled at the host edge (transcoded to and from protobuf via a
`FileDescriptorSet`); the authoritative contract remains protobuf, consistent with
[ADR 0009](./0009-protocol-buffers-wire-contracts.md).

### 5. Discovery, versioning, and scaling come from micro

Discovery (`$SRV.INFO/PING/STATS` and their `.<name>` and `.<name>.<id>`
variants), version reporting, and per-endpoint stats are provided by NATS micro,
so a service does not hand-roll a routing inventory. Endpoints subscribe on the
micro default queue group `q` so replicas share load; the queue group is
overridable at the service, group, or endpoint level.

### 6. The binding is a shared layer

The subject derivation, content-type negotiation, error-channel mapping, and
discovery wiring are one shared component, not reimplemented per service. Domain
crates inject only what is domain-specific: the typed request and response
messages, the subject prefix, and any outcome taxonomy layered on the error rule.

## Invariants

- A reply is a service error if, and only if, `Nats-Service-Error-Code` is
  present. Never infer success or failure by structurally deserializing the body.
- The method is carried by the subject; the request message type is a property of
  the endpoint, never parsed from the subject.
- The registered micro service exposes exactly the annotated `service`'s methods
  as endpoints.
- `Content-Type` is authoritative for payload encoding and is constrained by
  `ServiceOptions.content_type`.
- The authoritative message contract is protobuf; JSON is an edge encoding.

## Alternatives Considered

### Carry protobuf services over the JSON-RPC binding (ADR 0011)

Rejected: protobuf services are not JSON-RPC, so 0011's codec (envelope fields,
`Jsonrpc-*` headers, `id` semantics) does not apply. ADR 0011 is scoped to the
JSON-RPC family by its own terms.

### Let each protobuf service invent its own NATS mapping

Rejected for the reason ADR 0011 exists: independent mappings diverge, disagree on
success-versus-error, and lose structured errors. A single shared rule over the
shared options proto prevents that.

### Run real gRPC (HTTP/2) tunneled over NATS

Rejected: NATS is the transport under [ADR 0003](./0003-ai-protocol-transport-taxonomy.md),
and NATS micro already supplies request/reply, discovery, and error semantics.
Tunneling a second transport inside NATS adds framing and a dependency for no
gain. gRPC is referenced only as a naming idiom.

## Consequences

- First-party protobuf services get one discoverable, versioned, load-balanced
  NATS surface without hand-rolling routing or error signaling.
- Routing and health metering happen on the subject and the standard micro error
  header without decoding protobuf payloads.
- The `trogon.nats.micro.v1alpha1` options proto now has a governing wire
  contract; annotating a service is sufficient to define its NATS binding.
- [ADR 0017](./0017-decider-command-over-nats-binding.md) specializes this binding
  for decider commands; future first-party service APIs inherit it.

## References

- [`trogon.nats.micro.v1alpha1` options](https://github.com/TrogonStack/trogon-proto/blob/main/proto/trogon/nats/micro/v1alpha1/options.proto)
- [NATS Service API (ADR-32)](https://github.com/nats-io/nats-architecture-and-design/blob/main/adr/ADR-32.md)
- [ADR 0003: AI Protocol Transport Taxonomy](./0003-ai-protocol-transport-taxonomy.md)
- [ADR 0009: Protocol Buffers Wire Contracts](./0009-protocol-buffers-wire-contracts.md)
- [ADR 0011: JSON-RPC over NATS Binding](./0011-jsonrpc-over-nats-binding.md)
- [ADR 0017: Decider Command over NATS Binding](./0017-decider-command-over-nats-binding.md)

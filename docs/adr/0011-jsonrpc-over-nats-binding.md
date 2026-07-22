---
number: "0011"
slug: jsonrpc-over-nats-binding
status: accepted
date: 2026-06-23
---

# ADR#0011: JSON-RPC over NATS Binding

## Context

Several first-party [protocols](../glossary/protocol) are JSON-RPC 2.0 protocols carried over the [NATS](../glossary/nats)
backbone ([ADR#0003](./0003-ai-protocol-transport-taxonomy.md)): [ACP](../glossary/acp), [MCP](../glossary/mcp), and
[A2A](../glossary/a2a). [ADR#0003](./0003-ai-protocol-transport-taxonomy.md) states that the same
JSON-RPC lifecycle can run over stdio, a remote endpoint, or the internal
backbone, and [ADR#0004](./0004-protocol-and-transport-layering.md) places
request/response mapping, notifications, and protocol error semantics in the
protocol-dispatcher layer rather than in domain code.

Today each protocol maps onto NATS independently, and the mappings disagree even
within one protocol:

- The ACP command path (client to agent) strips the JSON-RPC envelope. The NATS
  body is the bare params struct, the method is the subject, and correlation rides
  in a transport [header](../glossary/headers). The reply is a bare result struct.
- The ACP callback path (agent to client) keeps the full JSON-RPC envelope in the
  body.
- MCP over NATS keeps the full JSON-RPC envelope in the body end to end.

Because the command path has no envelope and no explicit discriminator, success
and failure are told apart by attempting to deserialize the body as the success
type and, on failure, re-attempting as a JSON-RPC error. This is centralized on
the [JetStream](../glossary/jetstream) paths but absent on the core request/reply command handlers, where
the reply is typed only as the success type. A structured protocol error there
fails to deserialize and collapses into a generic internal error, discarding the
originating code and message. Authentication rejection is the clearest casualty:
the failure is indistinguishable from an unavailable agent.

NATS provides correlation (the reply subject for core request/reply, a transport
correlation header plus a dedicated response consumer for JetStream) and routing
(the subject). It does not provide success-versus-error semantics, and it routes
on subjects and acts on headers without ever reading the body. A useful binding
should let infrastructure route and make decisions from the subject and headers
without unmarshalling the payload, while keeping JSON-RPC semantics exact.

This requires one rule for any JSON-RPC protocol on the backbone, not an
ACP-specific patch.

## Decision

### 1. Bind JSON-RPC to NATS as a binary content-mode codec

A JSON-RPC message is mapped onto a NATS message by a lossless codec: the
control and correlation fields go to the subject and headers, the payload goes to
the body. This is binary content mode, the same shape CloudEvents uses for its
binary mode and gRPC uses for status in trailers. The on-NATS form is an encoding
of the message, not a different message.

| JSON-RPC field | NATS location | Rule |
| --- | --- | --- |
| `jsonrpc` (`"2.0"`) | none | Constant; omitted on the wire and re-injected on decode. |
| `method` | subject | Routed by the subject, per existing subject schemes. |
| `id` | header `Jsonrpc-Id` | JSON literal (see §3). Absent means notification (request) or `null` (response). |
| `params` | body | Request payload. |
| `result` | body | Success payload. Present when no `Jsonrpc-Error-Code` is set. |
| `error.code` | header `Jsonrpc-Error-Code` | Integer. Present only on errors; its presence is the success/error discriminator. |
| `error.message`, `error.data` | body | Human-readable and structured error detail. |

Correlation and routing metadata are a separate, protocol-agnostic [transport](../glossary/transport)
concern under [ADR#0004](./0004-protocol-and-transport-layering.md), not part of
this mapping.

Requests and notifications are addressed by the method subject. A response carries
no method; it is addressed to the requester's reply subject for core request/reply
or to the response stream for JetStream, correlated by the transport.

### 2. The codec's headers are authoritative

For the fields in the table above, the header (or subject) is the authoritative
value, not a denormalized copy of something in the body. Infrastructure routes on
the subject and acts on headers — an error counter or dead-letter router on
`Jsonrpc-Error-Code`, an index on `Jsonrpc-Id` — without unmarshalling the body.

The codec's headers are named `Jsonrpc-*` to mark them as projections of JSON-RPC
fields. The JSON-RPC `id` is one such field, and it is not the transport's
correlation key: it is client-chosen and unique only per connection (so it
collides across clients before a session exists at `initialize`), arbitrary string
ids are not subject-safe, and notifications and null-id responses have no usable
id. Correlation is a separate transport concern
([ADR#0004](./0004-protocol-and-transport-layering.md)) and is out of scope here.

### 3. Preserve `id` type with a JSON-literal header

The `id` is stored in `Jsonrpc-Id` as its JSON literal and recovered with a JSON
parse:

- number `42` -> `Jsonrpc-Id: 42` -> parse -> `42`
- string `"42"` -> `Jsonrpc-Id: "42"` -> parse -> `"42"`
- string `"abc"` -> `Jsonrpc-Id: "abc"` -> parse -> `"abc"`

The quotes in the stored literal carry the type, so a numeric `1` and a string
`"1"` are distinct on the wire. NATS header values permit double quotes (only CR
and LF are disallowed in a value), so no separate type header is needed. Encoding
with ASCII escaping keeps the value free of raw control characters.

`null` is represented by the absence of the `Jsonrpc-Id` header, disambiguated
from a notification by the message direction the subject already carries: on a
response, an absent `Jsonrpc-Id` means `id: null`; on a request, an absent
`Jsonrpc-Id` means a notification. Requests therefore use a non-null `id`,
consistent with JSON-RPC's own guidance.

### 4. Correctness is a codec round-trip invariant; edges reconstruct

The semantic-preservation guarantee reduces to one property of the shared codec:
for every valid JSON-RPC message `m`, `decode(encode(m))` equals `m`, including
the `id` type. This is property-tested and fuzzed (numbers, strings,
numeric-looking strings, large integers, null, unicode string ids, results,
errors, notifications).

Canonical JSON-RPC is reconstructed only at protocol edges — the remote
HTTP/WebSocket/SSE listeners and the stdio [bridges](../glossary/bridge). The on-NATS encoding is an
internal wire format; nothing external consumes the raw stream as JSON-RPC. The
edge holds the original typed `id` while awaiting the reply (correlated by the
transport), so live request/reply type fidelity holds independent of the header
encoding.

### 5. Signing covers the authoritative headers and the body

Because `Jsonrpc-Error-Code` and `Jsonrpc-Id` are authoritative and live only in
headers, the A2A signing scheme covers those headers in addition to the body.
Otherwise the outcome, error code, and id are tamperable on an otherwise signed
message. Redaction is unaffected: `result`, `message`, `data`,
and `params` remain in the body where the redaction pipeline operates.

### 6. The codec is a shared, protocol-agnostic layer

The codec is one shared component, not reimplemented per protocol. It owns the
field mapping and its inverse, the `id` encoding, success-versus-error
discrimination via `Jsonrpc-Error-Code` presence, correlation, notification semantics, mapping
of transport failures to JSON-RPC errors, and edge reconstruction. Domain
packages (`acp-nats`, `mcp-nats`, A2A) inject what is domain-specific: subject
routing (method plus context to subject), transport selection (core request/reply
versus JetStream durable), and typed params and results.

Per [ADR#0004](./0004-protocol-and-transport-layering.md), this is the
protocol-dispatcher and transport seam. This is governed by the JSON-RPC exception
in [ADR#0009](./0009-protocol-buffers-wire-contracts.md): ACP, MCP, and JSON-RPC
have protocol-defined JSON contracts and are not re-encoded as Protocol Buffers.
Working name for the package is `jsonrpc-nats`, following the
`<protocol>-<backbone>` pattern in
[ADR#0003](./0003-ai-protocol-transport-taxonomy.md) with JSON-RPC as the
protocol.

## Invariants

- `decode(encode(m))` equals `m` for every valid JSON-RPC message, id type
  included.
- `jsonrpc` is constant `"2.0"`; it is re-injected on decode. When an edge
  reconstructs a message from canonical JSON-RPC, a `jsonrpc` value other than
  `"2.0"` (or an absent one) is rejected outright rather than coerced, so a
  foreign or future version fails hard instead of being mis-parsed as `2.0`.
- A response is an error iff `Jsonrpc-Error-Code` (an integer) is present: an error
  response has a `message` body, a success response has a `result` body.
- A response with neither a `result` body nor a `Jsonrpc-Error-Code` is a protocol
  error.
- `Jsonrpc-Id` is the id's JSON literal and is purely an application-level
  correlation token: the server echoes it back into the response so the client can
  match the reply to its request. Its **presence is how a client asks for a
  reply**; its **absence means a notification** (request), and the server does not
  respond and does not invoke a request handler for it. Requests use a non-null id.
  A `null` id in a *response* is reserved for the one case where the server could
  not determine the request's id (a parse error / invalid request); it is never
  used for a success response, and there is no other situation that produces a
  `null`-id reply.
- The method is carried by the subject.
- The JSON-RPC `id` is not the transport *routing* key; routing a reply
  (`X-Req-Id`, the NATS reply inbox, the JetStream response consumer) is a separate
  transport concern. The two layers do not override each other: a reply inbox only
  says *where* a reply would go, not *whether* one is owed. A message with no
  `Jsonrpc-Id` is a notification and receives no reply **even if a reply inbox is
  present** — the absent id, not the inbox, is authoritative for the
  request-versus-notification decision. (A well-formed notification is published
  with no reply inbox in the first place.)

## Design Rules

- The success/error discriminator is the presence of the `Jsonrpc-Error-Code`
  header. Never infer it by structural deserialization of the body.
- Name the codec's headers `Jsonrpc-*` to mark JSON-RPC field projections. Do not
  place a transport or correlation field under `Jsonrpc-*`.
- Reconstruction to and from canonical JSON-RPC happens only at protocol edges,
  centralized in the shared codec, never ad hoc in domain code.
- Signing spans the authoritative `Jsonrpc-*` headers as well as the body.
- A batch request is unbundled at the edge into individual messages; the backbone
  carries one JSON-RPC message per NATS message.
- Model bidirectional protocols as a symmetric peer that can both send and serve.

## Alternatives Considered

### Keep the full JSON-RPC envelope in the body

The body carries the complete message and headers stay thin. This is the most
compatible option and the simplest codec, and it fixes the error-loss bug on its
own by keeping `result` exclusive-or `error` in the body. It was rejected because
infrastructure then cannot route or decide on success, error code, or id without
unmarshalling the body, which is the routing and middleware leverage this ADR
exists to provide. It remains the fallback if the codec proves too costly.

### Denormalize: full envelope in the body plus derived header copies

Keep the body authoritative and also project the control fields into headers as
derived, non-authoritative duplicates. This keeps compatibility and reversibility
while still exposing headers to middleware. It was rejected because every
projected field becomes a second home that can drift from the body and needs a
consistency check, and because middleware acting on a possibly stale or forged
derived header is unsound for security-adjacent decisions. With a shared codec
amortized across ACP, MCP, and A2A and real header consumers (error counters, id
indexes), a single authoritative home for each field is cleaner than maintaining
duplicates.

### Carry `id` with a separate type-tag header

Store `id` as a bare value plus a `Jsonrpc-Id-Type` header. Rejected in favor of
the self-describing JSON-literal encoding in §3, which uses one header and cannot
fall out of sync with a separate type tag.

## Open Implementation Questions

These are now resolved by the `jsonrpc-nats` transport seam:

- **The transport trait spanning core request/reply and JetStream durable
  delivery.** The shared core is a small set of free functions over the
  `trogon-nats` client traits, not a single bespoke trait: `merge_jsonrpc_headers`,
  `jsonrpc_request_raw` (byte-level request/reply with timeout), and
  `jsonrpc_publish[_with_timeout]`. They own header projection, the request/publish
  mechanics, and timeouts. The **typed decode is injected by the domain crate** —
  ACP decodes the generic `Message`, MCP decodes rmcp's `RxJsonRpcMessage` — so the
  shared core never depends on a protocol's message types.
- **How server-initiated streaming and JetStream keepalive ack compose.** They
  stay in the domain adapter, layered on top of the shared primitives rather than
  inside the shared core. MCP keeps a thin `rmcp::Transport` (Sink/Stream) adapter
  because rmcp fixes its transport shape; ACP's prompt notification stream and
  keepalive ack live in its runner. Both still send/publish through the shared
  functions, so the wire encoding is identical.

## Migration

- Generalize the existing `mcp-nats` NATS transport into the shared codec, moving
  it from full-envelope-in-body to the content-mode encoding.
- Prove the codec by migrating one ACP command end to end, starting with
  `authenticate`, the handler currently losing structured errors.
- Roll the remaining ACP command handlers onto the codec, then fold `mcp-nats`
  onto it so ACP and MCP share one transport and one wire mapping.
- Extend the A2A signing scheme to cover the authoritative `Jsonrpc-*` headers
  before the encoding is used on signed paths.
- Cut over per handler and per subject; a subject never carries the old and new
  encodings at the same time.

## Consequences

- Success-versus-error has one correct, tested implementation driven by the
  presence of `Jsonrpc-Error-Code`. The class of bugs where a structured error
  degrades to a
  generic internal error is eliminated.
- Infrastructure routes on subjects and acts on `Jsonrpc-Error-Code` and
  `Jsonrpc-Id` without unmarshalling the body.
- Each control field has exactly one home, so there is no body/header duplication
  to keep consistent.
- The body alone is not interpretable; a consumer reads headers to interpret it.
  This is acceptable because JetStream persists headers with the message.
- The codec is core infrastructure on the hot path; its correctness rests on the
  round-trip property test.
- The A2A signing scheme must cover the authoritative headers.
- ACP and MCP share one transport and wire mapping; A2A and future JSON-RPC
  protocols inherit the codec.
- The change introduces a new shared package and refactors the ACP NATS adapter,
  the ACP runner, and the MCP NATS transport. This is a deliberate short-term cost
  for a stable long-term seam.

## References

- [ADR#0003: AI Protocol Transport Taxonomy](./0003-ai-protocol-transport-taxonomy.md)
- [ADR#0004: Protocol and Transport Layering](./0004-protocol-and-transport-layering.md)
- [ADR#0009: Protocol Buffers Wire Contracts](./0009-protocol-buffers-wire-contracts.md)
- [JSON-RPC 2.0 Specification](https://www.jsonrpc.org/specification)

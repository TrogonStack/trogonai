---
number: "0011"
slug: jsonrpc-over-nats-binding
status: proposed
date: 2026-06-23
---

# ADR 0011: JSON-RPC over NATS Binding

## Context

Several first-party protocols are JSON-RPC 2.0 protocols carried over the NATS
backbone ([ADR 0003](./0003-ai-protocol-transport-taxonomy.md)): ACP, MCP, and A2A. ADR 0003 states that the same JSON-RPC
lifecycle can run over stdio, a remote endpoint, or the internal backbone, and
[ADR 0004](./0004-protocol-and-transport-layering.md) places request/response mapping, notifications, and protocol error
semantics in the protocol-dispatcher layer rather than in domain code.

Today each protocol maps onto NATS independently, and the mappings disagree even
within one protocol:

- The ACP command path (client to agent: `initialize`, `authenticate`,
  `session/new`, `session/prompt`, and similar) strips the JSON-RPC envelope. The
  NATS body is the bare params struct, the method is encoded in the subject, and
  correlation rides in the `req_id` header. The reply is a bare result struct.
- The ACP callback path (agent to client: `fs/read_text_file`,
  `session/request_permission`, `terminal/*`) keeps the full JSON-RPC envelope in
  the body.
- MCP over NATS keeps the full JSON-RPC envelope in the body end to end.

Because the command path has no envelope and no explicit discriminator, success
and failure are distinguished by attempting to deserialize the body as the success
type and, on failure, re-attempting as a JSON-RPC error. This structural
discrimination is centralized on the JetStream paths but is absent on the core
request/reply command handlers, where the reply is typed only as the success type.
A structured protocol error on those handlers fails to deserialize and collapses
into a generic internal error, discarding the originating error code and message.
Authentication rejection is the clearest casualty: the failure is
indistinguishable from an unavailable agent.

NATS provides correlation (the reply subject for core request/reply, the `req_id`
header plus a dedicated response consumer for JetStream) and routing (the subject).
It does not provide success-versus-error semantics. The fix is not to guess from
the body, and not to leave the whole envelope inline, but to bind each JSON-RPC
field to the part of the NATS message that fits it, with the success/error
discriminator made explicit.

This requires one rule for any JSON-RPC protocol on the backbone, not an
ACP-specific patch.

## Decision

### 1. Split the JSON-RPC message across the NATS message

A JSON-RPC message is bound to a NATS message across two planes: a control and
correlation plane (subject and headers) and a semantic payload plane (body). The
success-versus-error discriminator is explicit, in a header, and is never inferred
by deserializing the body.

| JSON-RPC field | NATS location | Rule |
| --- | --- | --- |
| `jsonrpc` (`"2.0"`) | header | Constant protocol-version marker. |
| `method` | subject | Routed by the subject, per existing subject schemes. |
| `id` | header | Correlation. Present on requests and responses; absent on notifications. |
| `params` | body | Request payload. |
| `status` (`ok` or `error`) | header | Authoritative success/error discriminator. New field, not in base JSON-RPC. |
| `result` | body | Success payload. Present if and only if `status` is `ok`. |
| `error.code` | header | Machine-routable error code. Present if and only if `status` is `error`. |
| `error.message`, `error.data` | body | Human-readable and structured error detail. |

A response body is therefore either the `result` value (`status: ok`) or the
`{ message, data }` error detail (`status: error`); the header selects which.

This is governed by the JSON-RPC exception in [ADR 0009](./0009-protocol-buffers-wire-contracts.md): ACP, MCP, and JSON-RPC
have protocol-defined JSON contracts and are not re-encoded as Protocol Buffers.

### 2. The NATS message is the unit of meaning; headers are authoritative for control fields

The interpretable unit is the whole NATS message: subject, headers, and body
together. The body alone is not self-describing, because `status`, `id`, `code`,
and `jsonrpc` live only in headers. A consumer must read `status` to interpret the
body. JetStream persists headers with the message, so a replayed message remains
fully interpretable; a consumer that reads only the body is incorrect by
construction.

Each field has exactly one home, per the table above. Fields are not duplicated
across planes, so there is no dual-source-of-truth to keep consistent.

Sensitive content stays in the body. `result`, `message`, and `data` are the
fields that can carry paths, identifiers, or internal detail, and they remain in
the body where the A2A redaction pipeline operates.

### 3. Signing must cover the control headers

Because `status`, `code`, and `id` carry meaning and live only in headers, the
A2A signing scheme must cover those headers, not only the body. Otherwise the
success/error outcome, the error code, and the correlation id are tamperable on an
otherwise signed message.

### 4. The JSON-RPC-to-NATS binding is a shared, protocol-agnostic layer

The binding is factored into one shared component rather than reimplemented per
protocol. The shared layer owns:

- the field split defined above and its inverse,
- success-versus-error discrimination via `status`, implemented and tested once,
- correlation between a request and its reply,
- notification (no-id, no-reply) semantics,
- mapping of transport failures (timeout, no responders) to JSON-RPC errors,
- reconstruction of a full JSON-RPC object at protocol edges.

The layer does not simply serialize an SDK envelope type into the body. SDK types
such as `rmcp::JsonRpcMessage` and `agent_client_protocol::Response` serialize
`jsonrpc`, `id`, `result`, and `error` inline; the layer extracts the control
fields into headers and places only `result` or `message`/`data` in the body, and
reassembles the full object when crossing to an edge that speaks JSON-RPC (the
remote HTTP/WebSocket/SSE listeners and the stdio bridges).

Per ADR 0004, this is the protocol-dispatcher and transport seam. Domain packages
(`acp-nats`, `mcp-nats`, A2A) inject what is genuinely domain-specific and stay
thin:

- subject routing: the mapping between a method (with its context, such as a
  session id) and a NATS subject,
- transport selection: core request/reply versus JetStream durable,
- typed params and results.

Working name for the shared package is `jsonrpc-nats`, following the
`<protocol>-<backbone>` pattern in ADR 0003 with JSON-RPC as the protocol.

## Invariants

- The `jsonrpc` header is `"2.0"`.
- `status` is `ok` or `error`.
- `status: ok` implies a `result` body and no `code` header.
- `status: error` implies a `code` header and a `message` body.
- An `id` header is present on every request and response, a response `id` equals
  its request `id`, and a notification has no `id` and receives no reply.
- The method is carried by the subject.

## Design Rules

- The success/error discriminator is the `status` header. Never infer it by
  structural deserialization of the body.
- A field has exactly one home, per the mapping table. Do not duplicate a field
  across header and body.
- Reconstruction to and from a full JSON-RPC object happens only at protocol
  edges, centralized in the shared layer, never ad hoc in domain code.
- Signing spans the control headers (`status`, `code`, `id`) as well as the body.
- Model bidirectional protocols as a symmetric peer that can both send requests
  and notifications and serve incoming ones. Do not split a JSON-RPC peer into
  fixed client and server roles when the protocol is bidirectional.

## Open Implementation Questions

These do not block the decision and are resolved during implementation:

- The concrete header names and namespace for `jsonrpc`, `id`, `status`, and
  `code`.
- Whether the shared layer exposes typed params and results or a canonical
  envelope value at its API surface, given it must field-split rather than
  serialize SDK envelope types directly.
- The exact transport trait that spans core request/reply and JetStream durable
  delivery.
- How server-initiated streaming (the ACP prompt notification stream plus its
  final response) and JetStream keepalive acknowledgement compose on top of the
  peer primitives without entering the shared core.

## Migration

- Generalize the existing `mcp-nats` NATS transport into the shared layer, adapting
  it from full-envelope-in-body to the field split.
- Prove the layer by migrating one ACP command end to end, starting with
  `authenticate`, the handler currently losing structured errors.
- Roll the remaining ACP command handlers onto the shared layer, then fold
  `mcp-nats` onto it so ACP and MCP share one transport and one wire mapping.
- Extend the A2A signing scheme to cover the control headers before the split is
  used on signed paths.
- Existing stripped or ad hoc JSON paths migrate when touched, consistent with
  the migration guidance in ADR 0009.

## Consequences

- Success-versus-error has one correct, tested implementation driven by an explicit
  header. The class of bugs where a structured protocol error silently degrades to
  a generic internal error is eliminated.
- Infrastructure can route, meter, and dead-letter on `status` and `code` without
  deserializing the body.
- The body is not independently interpretable; consumers must read the `status`
  header. This is acceptable because JetStream persists headers with the message.
- Edges that speak full JSON-RPC must assemble and disassemble the object from
  subject, headers, and body. The shared layer owns this so edges and domains do
  not reimplement it.
- The A2A signing scheme must be extended to cover control headers; otherwise the
  outcome, error code, and correlation id are tamperable.
- ACP and MCP share a transport and a wire mapping instead of maintaining separate
  bindings. A2A and future JSON-RPC protocols inherit the binding.
- The change introduces a new shared package and refactors the ACP NATS adapter,
  the ACP runner, and the MCP NATS transport. This is a deliberate short-term cost
  for a stable long-term seam.
- Any future package naming or restructuring follows the vocabulary in ADR 0003.

## References

- [ADR 0003: AI Protocol Transport Taxonomy](/adr/0003-ai-protocol-transport-taxonomy)
- [ADR 0004: Protocol and Transport Layering](/adr/0004-protocol-and-transport-layering)
- [ADR 0009: Protocol Buffers Wire Contracts](/adr/0009-protocol-buffers-wire-contracts)
- [JSON-RPC 2.0 Specification](https://www.jsonrpc.org/specification)

---
number: "0041"
slug: canonical-mcp-jsonrpc-bodies-over-nats
status: draft
date: 2026-07-29
---

# ADR#0041: Canonical MCP JSON-RPC Bodies over NATS

## Context

[ADR#0011](./0011-jsonrpc-over-nats-binding.md) defines a binary content-mode
encoding for JSON-RPC over [NATS](../glossary/nats). It removes the JSON-RPC
envelope from the body and makes the subject, `Jsonrpc-Id`, and
`Jsonrpc-Error-Code` authoritative.

The MCP 2026-07-28 transport model requires custom transports to preserve the
JSON-RPC message format and the per-request metadata model. It also states that
the body is the source of truth when a binding mirrors selected fields into
envelope metadata. SEP-2243 follows that rule for Streamable HTTP: `Mcp-Method`,
`Mcp-Name`, and `Mcp-Param-*` are routing projections whose values must agree
with the JSON-RPC body.

The content-mode encoding cannot satisfy those rules. Its body is not a JSON-RPC
message, protocol metadata can be lost when a proxy reconstructs typed requests,
and a future pass-through intermediary cannot forward the body without knowing
our private reconstruction rules. The private `Jsonrpc-Id` header also has no MCP
equivalent and duplicates a value that MCP requires in the body.

## Decision

### 1. The MCP NATS body is canonical JSON-RPC

Every MCP request, notification, success response, and error response carries
its complete JSON-RPC 2.0 object in the NATS body. The body includes `jsonrpc`,
`id` when the message kind permits it, `method`, and exactly one of `params`,
`result`, or `error` as defined by JSON-RPC and MCP.

The body is authoritative. A consumer can forward it without reconstructing the
protocol envelope or understanding the method schema.

### 2. Routing data is a derived projection

The NATS subject remains a projection of the request or notification method so
NATS can route without parsing the body. A receiver must reject a subject method
that disagrees with the body method.

MCP publishers do not emit `Jsonrpc-Id` or `Jsonrpc-Error-Code`. A decoder may
temporarily accept those legacy headers only when they agree with the canonical
body. They never override the body.

Known MCP methods retain their readable subject suffixes. Custom and future
methods use a reversible, collision-free fallback suffix rather than being
rejected by a closed method table.

### 3. Per-request metadata remains in the body

MCP `_meta` remains under request or notification `params`. When the SDK moves
that metadata into a request context before handler dispatch, a proxy must put it
back into the forwarded message extensions so serialization restores the same
`params._meta` value.

### 4. MCP HTTP routing headers survive the proxy boundary

The HTTP-to-NATS proxy carries only this allowlist into NATS headers:

- `MCP-Protocol-Version`
- `Mcp-Method`
- `Mcp-Name`
- `Mcp-Param-*`

Header names are matched case-insensitively. Authentication, cookies, the removed
`Mcp-Session-Id`, and arbitrary HTTP headers are not forwarded.

These headers remain derived metadata. A schema-aware boundary validates them
against the canonical body. A schema-unaware intermediary forwards and otherwise
ignores them. No header may alter the JSON-RPC message reconstructed from the
body.

### 5. Other protocols keep the existing codec during migration

Upon acceptance, this decision supersedes ADR#0011 for MCP only. ACP and A2A
continue using its content-mode codec until their consumers, signing rules, and
rollout plans are evaluated separately. The shared `jsonrpc-nats` package exposes
the two wire modes explicitly so selecting one is deliberate.

## Invariants

- The NATS body alone deserializes as the same MCP JSON-RPC message that entered
  the transport.
- Request and response ids retain their JSON type and value without consulting a
  NATS header.
- A subject method or legacy projection that disagrees with the body is rejected.
- `params._meta` survives an HTTP proxy to NATS round trip.
- Allowed `Mcp-*` routing headers survive the proxy, while unrelated HTTP headers
  do not.
- Existing ACP and A2A wire bytes do not change as a consequence of the MCP
  migration.

## Consequences

- MCP messages can pass through generic JSON-RPC infrastructure and custom
  transports without a private reconstruction step.
- NATS routing remains cheap because the method is still projected into the
  subject.
- Duplicate routing metadata requires mismatch validation, but it is never an
  alternate source of protocol truth.
- Existing MCP nodes using the content-mode body are not wire-compatible with
  canonical-body publishers. Deployment must be coordinated or use a versioned
  subject during a rolling migration.
- The generic content-mode codec remains until ACP and A2A make their own
  migration decisions.

## References

- [ADR#0011: JSON-RPC over NATS Binding](./0011-jsonrpc-over-nats-binding.md)
- [ADR#0004: Protocol and Transport Layering](./0004-protocol-and-transport-layering.md)
- [MCP draft transport model](https://modelcontextprotocol.io/specification/draft/basic/transports)
- [SEP-2243: HTTP Header Standardization](https://modelcontextprotocol.io/seps/2243-http-standardization)
- [JSON-RPC 2.0 Specification](https://www.jsonrpc.org/specification)

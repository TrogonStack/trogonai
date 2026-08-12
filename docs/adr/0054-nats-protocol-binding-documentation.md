---
number: "0054"
slug: nats-protocol-binding-documentation
status: draft
date: 2026-08-08
---

# ADR#0054: NATS Protocol Binding Documentation

## Context

This repository routes external AI protocols — Model Context Protocol (MCP),
Agent Client Protocol (ACP), and Agent2Agent (A2A) — over the NATS backbone
([ADR#0003](./0003-ai-protocol-transport-taxonomy.md)). The protocol-to-NATS
adapters already exist (`mcp-nats`, `acp-nats`, `a2a-nats`, and their transport
bridges) and share a method-oriented subject grammar:

```text
mcp.server.{server_id}.tools.call
mcp.server.{server_id}.notifications.initialized
mcp.client.{client_id}.sampling.create_message
a2a.agents.{agent_id}.message.send
acp.{...}.{method}
```

The grammar is currently described informally and unevenly: some crates document
their subjects in a README, others only in code. There is no single
machine-readable artifact that states, for a given protocol, which NATS subject
carries which message and what payload that message holds. This is a gap for
onboarding, review, tooling, change control, and every language workspace in the
polyglot layout ([ADR#0005](./0005-polyglot-workspace-layout.md)).
The question this ADR answers is: **how do we statically
document a NATS subject together with its payload, for protocols whose payload
schemas we do not own.**

Four facts constrain the answer:

1. **The payload schemas are externally owned.** MCP publishes JSON Schema
   (source of truth: TypeScript). ACP publishes versioned JSON Schema
   (`schema/v1/schema.json`, source of truth: Rust). A2A's normative source is
   Protobuf (`a2a.proto`); its JSON artifact is non-normative. We must reference
   these, never redefine them, or we drift from upstream on every version bump.

2. **All three are JSON-RPC 2.0.** They share one message shape — request,
   response, notification — with `method`, `params`, `id`, `result`/`error`.
   Carrying them over NATS is therefore one problem ("JSON-RPC 2.0 over NATS"),
   not three.

3. **[ADR#0009](./0009-protocol-buffers-wire-contracts.md) already settles the
   encoding.** Protocol-defined JSON contracts
   (MCP, ACP, JSON-RPC) are an explicit exception to the Protocol Buffers default.
   The payload stays JSON-RPC; we do not protobuf-ify it. Protobuf-as-source-of-
   truth is not an option here because two of the three payloads are not
   protobuf-native and none are ours to own.

4. **The wire codec is decided elsewhere.**
   [ADR#0056](./0056-canonical-jsonrpc-bodies-over-nats.md) (draft) selects
   canonical full-envelope JSON-RPC bodies for MCP, ACP, and A2A: the body is
   authoritative; the subject method and `Jsonrpc-*` headers are derived,
   non-authoritative projections. That ADR supersedes
   [ADR#0011](./0011-jsonrpc-over-nats-binding.md)'s content-mode decision and
   absorbs [ADR#0041](./0041-canonical-mcp-jsonrpc-bodies-over-nats.md) upon
   acceptance. This ADR documents the binding surface; it does not re-decide the
   codec.

## Decision

Document each NATS protocol binding with an **AsyncAPI 3.x document**, one per
protocol, treating NATS as a transport binding for JSON-RPC 2.0.

AsyncAPI is chosen because it is the one widely-supported standard that
dissociates the message description from the protocol binding — which maps
directly onto our split: **the subject is ours, the payload is theirs.** We do
not invent a new specification; we extend AsyncAPI with a small `x-nats-*`
vocabulary for the backbone specifics its NATS binding does not yet cover.

The decision has four parts.

### 1. Subject and operation

- Each JSON-RPC `method` maps to one AsyncAPI channel whose `address` is the NATS
  subject. The subject grammar is the canonical
  `{prefix}.v{major}.<routing-segment>.{terminal...}` of
  [ADR#0055](./0055-nats-subject-design-jsonrpc-bindings.md). The terminal is the
  method under that binding's method-to-terminal mapping, which is **not** a
  plain `/`-to-`.` rewrite: MCP folds case and escapes unknown methods
  (`sampling/createMessage` → `sampling.create_message`,
  unknown → `custom.{base64url}`), and ACP drops the segment the routing segment
  already carries (`session/prompt` → `prompt`).
- Each binding's AsyncAPI document **publishes that mapping**, which [ADR#0055](0055-nats-subject-design-jsonrpc-bindings.md)
  requires to be total and bidirectional. It is the artifact that makes the
  mapping reviewable instead of code-only.
- `parameters` describe the variable tokens (`peer_id`, tenant `prefix`).
- Request/response uses AsyncAPI `reply`; notifications are send-only operations.

### 2. Payload

- `messages[].payload` references the **upstream** schema via `$ref` and
  `schemaFormat`. It is never hand-redefined.
  - MCP → official MCP JSON Schema.
  - ACP → `schema/v1/schema.json`, pinned to the `meta.json` protocol version.
  - A2A → its published JSON artifact, pinned to a commit; the Protobuf file
    remains the normative upstream definition.
- The **on-wire body shape belongs to the codec ADR, not to this document.**
  How a JSON-RPC message is split between subject, headers, and body is decided
  by [ADR#0056](./0056-canonical-jsonrpc-bodies-over-nats.md) (canonical
  full-envelope body for MCP, ACP, and A2A; body authoritative; disagreeing
  subject or header projections rejected). Each protocol's AsyncAPI document
  describes the resulting message. A change of codec is a change to that ADR,
  never a silent edit to the AsyncAPI document. Because the body is a complete
  upstream JSON-RPC message, `messages[].payload` `$ref`s resolve directly
  against vendored upstream schemas with no hand-derived sub-schema.

### 3. Correlation

- Correlation rules are **owned by
  [ADR#0055](./0055-nats-subject-design-jsonrpc-bindings.md)** (Correlation and
  headers), not restated here. In outline: core request/reply correlates on the
  NATS reply inbox; a durable hop correlates on a transport token, which is a
  separate header where the peer supplies the `id` and MAY be the JSON-RPC `id`
  itself where the transport mints it. Which of the two a given binding uses is
  stated in that binding's AsyncAPI document, not assumed.
- The JSON-RPC `id` is application-layer, travels authoritatively in the
  canonical body ([ADR#0056](./0056-canonical-jsonrpc-bodies-over-nats.md)), and
  is projected to a non-authoritative `Jsonrpc-Id`. The response `id` MUST equal
  the request `id`.
- No per-request identifier is a subject token, `id` or correlation token alike.
  A durable response channel's `address` is scoped to the bounded entity
  (session, connection, task) and the correlation token demultiplexes requests
  *within* it, so an AsyncAPI channel never carries a `req_id` parameter.
  Long-lived session and connection identifiers are ordinary subject tokens under
  [ADR#0055](0055-nats-subject-design-jsonrpc-bindings.md)'s cardinality rule.
- Notifications carry no `id` and no reply subject (fire-and-forget).
- Errors are JSON-RPC error responses delivered on the same response channel as
  successes (the reply inbox for core request/reply, the response consumer for
  JetStream); the binding does not invent a parallel NATS-level error channel.

### 4. `x-nats-*` extension vocabulary

A single extension vocabulary, shared across all protocol documents, carries what
AsyncAPI's NATS binding (v0.1.0) omits:

- `x-nats-stream` — JetStream stream binding, when the subject is persisted.
- `x-nats-version` — the binding's subject-contract version (the `v{major}`
  token of [ADR#0055](./0055-nats-subject-design-jsonrpc-bindings.md)), distinct
  from the upstream protocol version.
- `x-nats-delivery` — core request/reply vs JetStream semantics.
- `x-nats-headers` — the header contract:
  - Derived codec projections from
    [ADR#0056](./0056-canonical-jsonrpc-bodies-over-nats.md): `Jsonrpc-Id`,
    `Jsonrpc-Error-Code` (non-authoritative; body wins on disagreement).
  - Negotiated protocol version, named by the rule
    `{PROTOCOL}-Protocol-Version` from
    [ADR#0055](./0055-nats-subject-design-jsonrpc-bindings.md):
    `MCP-Protocol-Version`, `ACP-Protocol-Version`, `A2A-Protocol-Version`.
    ACP and A2A emission is deferred until a durable stream must survive a
    protocol version bump; MCP already emits `MCP-Protocol-Version`. The
    JSON-RPC `"2.0"` literal MUST NOT appear as a header; it lives in the body.
  - The durable-hop correlation token, per [ADR#0055](0055-nats-subject-design-jsonrpc-bindings.md): a distinct `Trogon-Req-Id` where
    the peer supplies the JSON-RPC `id` (A2A), or `Jsonrpc-Id` itself where the
    transport mints it (ACP). The document states which; it does not assume both.
  - Content type, W3C `traceparent`/`tracestate`, and large-payload claim-check
    references.

The same vocabulary is reused, not re-specified, across MCP, ACP, and A2A. The
protocols' payloads stay independently governed; only the NATS subject conventions
are unified.

## Layering

This documentation sits at the transport/backbone boundary, consistent with
[ADR#0004](./0004-protocol-and-transport-layering.md):

- The **transport adapter** owns subjects, headers, reply inboxes, and timeouts.
  The AsyncAPI document is the static description of that adapter's surface.
- The **protocol dispatcher** owns request/response mapping, notifications, and
  protocol error semantics. The AsyncAPI `$ref` points at the schema the
  dispatcher already speaks.
- **Domain code stays protocol-agnostic.** The AsyncAPI document describes the
  edge, not the domain.

## Artifact Governance

- Store the AsyncAPI documents and a vendored, pinned copy of each upstream schema
  under a versioned location alongside the proto sources
  ([ADR#0009](./0009-protocol-buffers-wire-contracts.md) names `proto/` for
  protobuf; the AsyncAPI documents and vendored JSON Schemas need an analogous
  home — selecting it is an open question below).
- Treat an upstream schema bump as a reviewed change: re-pin the vendored schema,
  re-resolve `$ref`s, and review the diff. Non-normative upstream artifacts
  (A2A JSON) are pinned to a commit precisely because their stability is not
  guaranteed.

## Consequences

- One artifact per protocol documents subject and payload together, machine-
  readable, with tooling for rendered docs, mocking, and contract checks.
- The repository never redefines an externally-owned schema; it references and
  pins it, so upstream drift surfaces as a reviewable change.
- The existing subject grammar and reply-inbox behavior become a written contract
  instead of scattered crate-local prose and code.
- A new protocol added to the backbone follows the same JSON-RPC-over-NATS
  binding rules and `x-nats-*` vocabulary rather than a bespoke scheme.
- The cost is a constant per-message overhead (the `method` duplicated into the
  subject and, where a codec keeps the full envelope in the body, the envelope
  itself), accepted in exchange for a faithful, bridgeable transport binding.

## Open Questions

These are intentionally unresolved and are the work to build around this ADR:

- The on-disk home and naming for AsyncAPI documents and vendored upstream
  schemas, and whether documents are hand-authored or generated.
- The exact `x-nats-*` field set and its JSON Schema.
- The A2A binding (`a2a-nats`) is implemented, but its AsyncAPI document is not
  yet authored; writing it may reveal gaps the MCP/ACP documents do not.
- Whether contract checks (e.g. validating live subjects against the AsyncAPI
  document) run in CI, and against what.

## References

- [ADR#0003: AI Protocol Transport Taxonomy](./0003-ai-protocol-transport-taxonomy.md)
- [ADR#0004: Protocol and Transport Layering](./0004-protocol-and-transport-layering.md)
- [ADR#0005: Polyglot Workspace Layout](./0005-polyglot-workspace-layout.md)
- [ADR#0009: Protocol Buffers Wire Contracts](./0009-protocol-buffers-wire-contracts.md)
- [ADR#0011: JSON-RPC over NATS Binding](./0011-jsonrpc-over-nats-binding.md)
- [ADR#0041: Canonical MCP JSON-RPC Bodies over NATS](./0041-canonical-mcp-jsonrpc-bodies-over-nats.md)
- [ADR#0055: NATS Subject Design for JSON-RPC Protocol Bindings](./0055-nats-subject-design-jsonrpc-bindings.md)
- [ADR#0056: Canonical JSON-RPC Bodies over NATS](./0056-canonical-jsonrpc-bodies-over-nats.md)
- [AsyncAPI 3.0 specification](https://www.asyncapi.com/docs/reference/specification/v3.0.0)
- [AsyncAPI NATS bindings](https://github.com/asyncapi/bindings/blob/master/nats/README.md)
- [JSON-RPC 2.0 specification](https://www.jsonrpc.org/specification)
- [Model Context Protocol specification](https://modelcontextprotocol.io/specification)
- [Agent Client Protocol schema](https://agentclientprotocol.com/protocol/schema)
- [Agent2Agent (A2A) specification](https://a2a-protocol.org/latest/specification/)

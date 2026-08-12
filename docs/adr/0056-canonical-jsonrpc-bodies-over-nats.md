---
number: "0056"
slug: canonical-jsonrpc-bodies-over-nats
status: draft
date: 2026-08-09
---

# ADR#0056: Canonical JSON-RPC Bodies over NATS

## Context

[ADR#0011](./0011-jsonrpc-over-nats-binding.md) (accepted) binds JSON-RPC to NATS
as a binary content-mode codec: the body carries only `params`, `result`, or
error detail; `method` rides in the subject; `id` and error code live in
authoritative `Jsonrpc-*` headers. That split blocks three things this repository
now needs:

1. **Signing.** Signed A2A paths must cover both the body and the authoritative
   headers (`Jsonrpc-Id`, `Jsonrpc-Error-Code`), which is why `a2a-nats/src/wire.rs`
   blocks signed traffic today. Content mode splits the message across a body and
   two headers that a signature would have to co-sign; a canonical envelope
   collapses that to a single payload digest.
   This does **not** make a signed NATS request one byte range:
   [ADR#0051](./0051-fully-bound-request-signing.md) (accepted) binds the concrete
   subject and the operation as required claims alongside the payload digest. The
   gain here is narrower and still decisive — the digest covers a complete
   JSON-RPC message, so no authoritative value sits outside the signed payload.
2. **Upstream schema `$ref`.** [ADR#0054](./0054-nats-protocol-binding-documentation.md)
   documents bindings with AsyncAPI payloads that reference whole JSON-RPC
   messages. A content-mode body is `params` alone, so the AsyncAPI payload would
   need a hand-derived sub-schema, which [ADR#0054](0054-nats-protocol-binding-documentation.md) forbids.
3. **Envelope reconstruction in bridges.** Stdio and HTTP bridges currently
   hand-assemble `{jsonrpc, id, result}` because the NATS body is not a complete
   message. This is *envelope* reconstruction and is unrelated to
   [ADR#0021](./0021-typed-decode-over-passthrough-forwarding.md), which rejected
   *payload* passthrough (forwarding `params` as an untyped `Value` instead of
   decoding it). [ADR#0021](0021-typed-decode-over-passthrough-forwarding.md) is unaffected by this ADR; see §7.

[ADR#0041](./0041-canonical-mcp-jsonrpc-bodies-over-nats.md) (draft) already
moves MCP to canonical full-envelope bodies with non-authoritative header
projections, but it is scoped to MCP only and leaves ACP and A2A on content
mode. Amending 0041 is not sufficient: the codec must be one rule for every
JSON-RPC protocol on the backbone.

MCP is already on the canonical path in `jsonrpc-nats`. ACP and A2A still call
the legacy content-mode `encode`/`decode`. The shared package already validates
that derived `Jsonrpc-*` headers agree with the body when present; the canonical
encoder today emits an empty `HeaderMap`, which breaks A2A gateway paths that
read `Jsonrpc-Id` from headers. Emitting derived headers is part of making the
canonical path the only path.

## Decision

### 1. One canonical body for MCP, ACP, and A2A

Every JSON-RPC request, notification, success response, and error response on
the NATS backbone carries its complete JSON-RPC 2.0 object in the NATS body.
The body includes `jsonrpc` (`"2.0"`), `id` when the message kind permits it,
`method` on requests and notifications, and exactly one of `params`, `result`,
or `error` as defined by JSON-RPC and the protocol.

The body is authoritative. A consumer can forward it without reconstructing the
protocol envelope or understanding the method schema.

Upon acceptance, this ADR supersedes
[ADR#0011](./0011-jsonrpc-over-nats-binding.md) and absorbs
[ADR#0041](./0041-canonical-mcp-jsonrpc-bodies-over-nats.md).

**This also reverses [ADR#0022](./0022-canonical-acp-wire-methods-on-nats.md).**
[ADR#0022](0022-canonical-acp-wire-methods-on-nats.md) rejected canonical ACP method names on the NATS leg, and its first
premise was that "the content-mode codec never serializes the method ... there is
no method field on the NATS wire to make self-describing." Under this ADR there
is one: the canonical body carries `method` verbatim. [ADR#0022](0022-canonical-acp-wire-methods-on-nats.md) set the bar for
any reversal — "prove a concrete benefit at the byte level or in operations, not
vocabulary aesthetics" — and the benefits here are byte-level and operational,
not aesthetic:

- The body must validate against the upstream JSON Schema unmodified
  ([ADR#0054](./0054-nats-protocol-binding-documentation.md)). A body carrying
  `"method":"prompt"` fails ACP's own schema.
- A bridge can forward the envelope unmodified only if the body it forwards is
  already the message the peer sent.
- The payload digest under §3 covers the method, so a truncated method means a
  signature over a message that was never sent.

[ADR#0022](0022-canonical-acp-wire-methods-on-nats.md)'s actual holding is preserved: the **subject token** vocabulary stays
NATS-native and is not re-spelled to match the spec. What changes is that the
subject token is now a projection of the method rather than a replacement for it
([ADR#0055](./0055-nats-subject-design-jsonrpc-bindings.md), Method-to-terminal
mapping). The two vocabularies [ADR#0022](0022-canonical-acp-wire-methods-on-nats.md) refused to maintain in parallel are now
one mapping with one source of truth.

### 2. Subject and headers are derived, non-authoritative projections

- The NATS subject remains a projection of the request or notification method so
  NATS can route without parsing the body
  ([ADR#0055](./0055-nats-subject-design-jsonrpc-bindings.md)). A receiver MUST
  reject a request or notification whose subject terminal is not the projection
  of the body method under that binding's method-to-terminal mapping ([ADR#0055](0055-nats-subject-design-jsonrpc-bindings.md),
  Method-to-terminal mapping). Agreement is defined by the mapping, not by string
  equality: MCP's `sampling/createMessage` correctly projects to the terminal
  `sampling.create_message`. Responses carry no method and are exempt; they are
  routed by reply inbox or by an entity-scoped response subject.
- The body carries the protocol's own method string verbatim. A binding MUST NOT
  write the projected terminal into the body in place of the real method.
- Publishers MUST emit `Jsonrpc-Id` and `Jsonrpc-Error-Code` as derived
  projections of the body (`id` as its JSON literal; error code present only on
  errors). They never override the body. Decoders MUST reject a header that
  disagrees with the body when the header is present.
- Infrastructure MAY act on those headers for cheap routing and metrics. Security-
  adjacent decisions that require the authoritative value MUST read the body (or
  verify a signature over it).

`Jsonrpc-Id` encoding retains the JSON-literal rule from [ADR#0011](0011-jsonrpc-over-nats-binding.md) §3 so numeric
and string ids stay distinct. Absence of `Jsonrpc-Id` still means notification
(request) or `id: null` (response), disambiguated by message direction.

### 3. The payload digest covers the whole message

[ADR#0051](./0051-fully-bound-request-signing.md) owns the signing scheme and is
unchanged by this ADR. Its required binding claims over NATS stay exactly as
written: the concrete subject, the operation, the payload digest, and the time
window.

What this codec changes is only the **payload digest input**. Because the body
alone is the complete JSON-RPC message, the digest over the raw payload bytes
covers every authoritative value. Derived `Jsonrpc-*` headers are therefore not
signed material — a verifier re-derives them from the body or ignores them — and
content mode's requirement to co-sign authoritative headers is retired with that
codec. That requirement is what `a2a-nats/src/wire.rs` cites when it blocks
signed traffic, so migrating A2A to canonical is what unblocks it.

Two consequences of [ADR#0051](0051-fully-bound-request-signing.md) that this ADR does **not** relax:

- The subject stays inside the signed binding, so a body that agrees with itself
  is not sufficient. A verifier still checks the published subject.
- Because the subject is bound, rewriting it in flight breaks signatures.
  [ADR#0055](0055-nats-subject-design-jsonrpc-bindings.md)'s Versioning posture records the matching constraint:
  `subject_transform` bridging is unavailable for signed subtrees.

### 4. Correlation stays a transport concern

Request/response correlation remains outside the codec.
[ADR#0055](./0055-nats-subject-design-jsonrpc-bindings.md) (Correlation and
headers) owns the rules, including which token correlates a durable hop and when
a binding may correlate on the JSON-RPC `id` rather than a separate `Trogon-Req-Id`.
This ADR adds only that the `id` is application-level, travels authoritatively in
the body, and is projected to `Jsonrpc-Id`. `Nats-Msg-Id` stays reserved for
JetStream deduplication.

### 5. MCP-specific rules carried from [ADR#0041](0041-canonical-mcp-jsonrpc-bodies-over-nats.md)

These remain in force for MCP and are unchanged by the ACP/A2A migration:

- MCP `_meta` remains under request or notification `params`. Proxies that move
  metadata into request context before dispatch MUST restore it so serialization
  yields the same `params._meta`.
- The HTTP-to-NATS proxy allowlist stays: `MCP-Protocol-Version`, `Mcp-Method`,
  `Mcp-Name`, `Mcp-Param-*`. `Mcp-Session-Id` and unrelated HTTP headers are not
  forwarded. Those headers remain derived metadata validated against the body.

### 6. Shared package; legacy content mode removed after migration

The shared `jsonrpc-nats` package owns encode/decode. After ACP and A2A migrate
to the canonical APIs, the legacy content-mode `encode`/`decode` are removed (or
deprecated with a removal milestone). A bridge that hand-builds an envelope
forwards the body unmodified instead, where that assembly is redundant.

### 7. What this ADR does not change

- **Typed payload decode stays.**
  [ADR#0021](./0021-typed-decode-over-passthrough-forwarding.md) (accepted) keeps
  typed decode of `params` and `result` rather than untyped `Value` forwarding,
  for boundary validation before durable streams. This ADR changes the *envelope*
  a bridge forwards, never whether the payload inside it is validated. "The bridge
  no longer reassembles `{jsonrpc, id, result}`" does not mean "the bridge no
  longer decodes `params`." Re-evaluating [ADR#0021](0021-typed-decode-over-passthrough-forwarding.md) remains governed by its own
  Consequences section.
- **[ADR#0017](0017-aauth-agent-authentication.md)'s AAuth denial code stays.** The gateway still replies with JSON-RPC
  error `-32118` on an AAuth denial
  ([ADR#0017](./0017-aauth-agent-authentication.md) §4). What changes is where the
  authority sits. Under [ADR#0011](0011-jsonrpc-over-nats-binding.md) the code was authoritative *in the header*; under
  this ADR the body is authoritative and `Jsonrpc-Error-Code` is a derived
  projection. Because a denial is a security decision, a client MUST take the code
  from the body per §2. Infrastructure MAY still read the header to route or meter
  denials without parsing. The publisher emits both and they agree, so this is a
  reader-side rule, not a wire change.

## Invariants

- The NATS body alone deserializes as the same JSON-RPC message that entered the
  transport, method string included. ACP's current truncation of `session/prompt`
  to `prompt` violates this and is listed for correction in [ADR#0055](0055-nats-subject-design-jsonrpc-bindings.md)'s
  Conformance section.
- `decode_canonical(encode_canonical(m))` equals `m` for every valid message, id
  type included.
- A `Jsonrpc-*` projection that disagrees with the body is rejected, as is a
  subject terminal that is not the body method's projection under the binding's
  method-to-terminal mapping.
- `"jsonrpc":"2.0"` travels in the body; it is not duplicated into a header.
- MCP `params._meta` and allowlisted `Mcp-*` / `MCP-Protocol-Version` headers
  survive an HTTP proxy to NATS round trip.

## Consequences

- MCP, ACP, and A2A share one wire body shape, so payload tooling, validators,
  bridges, and AsyncAPI `$ref`s are shared.
- A2A per-message signing is unblocked: one authoritative byte range.
- Canonical encoder must emit derived `Jsonrpc-*` headers before A2A migrates, or
  the gateway paths that read `Jsonrpc-Id` from headers break.
- Existing ACP and A2A content-mode publishers are not wire-compatible with
  canonical-body publishers; cutover is coordinated while the binding is still
  pre-stable ([ADR#0055](./0055-nats-subject-design-jsonrpc-bindings.md)
  Versioning Posture).
- [ADR#0011](0011-jsonrpc-over-nats-binding.md)'s content-mode decision and [ADR#0041](0041-canonical-mcp-jsonrpc-bodies-over-nats.md)'s MCP-only scope are retired once
  this ADR is accepted.

## References

- [ADR#0003: AI Protocol Transport Taxonomy](./0003-ai-protocol-transport-taxonomy.md)
- [ADR#0004: Protocol and Transport Layering](./0004-protocol-and-transport-layering.md)
- [ADR#0009: Protocol Buffers Wire Contracts](./0009-protocol-buffers-wire-contracts.md)
- [ADR#0011: JSON-RPC over NATS Binding](./0011-jsonrpc-over-nats-binding.md)
- [ADR#0017: AAuth Agent Authentication](./0017-aauth-agent-authentication.md)
- [ADR#0021: Typed Decode over Passthrough Forwarding](./0021-typed-decode-over-passthrough-forwarding.md)
- [ADR#0022: Canonical ACP Method Vocabulary in the NATS Layer (Rejected)](./0022-canonical-acp-wire-methods-on-nats.md)
- [ADR#0041: Canonical MCP JSON-RPC Bodies over NATS](./0041-canonical-mcp-jsonrpc-bodies-over-nats.md)
- [ADR#0051: Fully Bound Request Signing](./0051-fully-bound-request-signing.md)
- [ADR#0054: NATS Protocol Binding Documentation](./0054-nats-protocol-binding-documentation.md)
- [ADR#0055: NATS Subject Design for JSON-RPC Protocol Bindings](./0055-nats-subject-design-jsonrpc-bindings.md)
- [JSON-RPC 2.0 Specification](https://www.jsonrpc.org/specification)
- [MCP draft transport model](https://modelcontextprotocol.io/specification/draft/basic/transports)
- [SEP-2243: HTTP Header Standardization](https://modelcontextprotocol.io/seps/2243-http-standardization)

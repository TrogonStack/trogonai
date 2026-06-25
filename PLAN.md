# Plan: Adopt ADR 0011 — JSON-RPC over NATS Binding

Implements [ADR 0011](./docs/adr/0011-jsonrpc-over-nats-binding.md) across the
Rust workspace: one shared, protocol-agnostic codec that binds any JSON-RPC 2.0
protocol (ACP, MCP, A2A) onto NATS in **binary content mode** — control and
correlation fields project to the subject and `Jsonrpc-*` headers, the payload
stays in the body, and success-versus-error is decided by the presence of the
`Jsonrpc-Error-Code` header, never by structural deserialization.

## Completed

- **Codec crate `jsonrpc-nats`** with `encode`/`decode`, the `Jsonrpc-Id` JSON
  literal codec, the `Jsonrpc-Error-Code` discriminator, edge reconstruction
  (`to_json_value`/`from_json_value`), and the `decode(encode(m)) == m`
  round-trip property tests. Unsupported/absent `jsonrpc` version fails hard
  (`CodecError::UnsupportedVersion`).
- **MCP** runs on the shared codec end to end (`mcp-nats/src/transport.rs`,
  `wire.rs`); canonical JSON-RPC is reconstructed only at the stdio and
  Streamable-HTTP/SSE edges.
- **ACP** runs on the codec: global core req/reply handlers, all JetStream
  session commands including `prompt` (`handle_js`), the `cancel` notification,
  and the full agent→client callback path (`rpc_reply` content-mode replies).
  The `authenticate` error-loss bug is fixed — structured agent errors survive
  via the header discriminator instead of collapsing to `-32603`.
- **A2A headers**: `a2a-nats/src/{client,server}/wire.rs` project `id` →
  `Jsonrpc-Id` and `error.code` → `Jsonrpc-Error-Code` through the shared codec;
  subject-based dispatch (`A2aMethod::from_subject`) retained. Redaction is
  unaffected (`result`/`message`/`data`/`params` stay in the body).
- **Shared transport** (`jsonrpc-nats/src/transport.rs`): ACP and MCP both run
  on the shared `merge_jsonrpc_headers`, `jsonrpc_request_raw` (byte-level
  request/reply with timeout), and `jsonrpc_publish[_with_timeout]`. A2A's
  duplicated header-merge now delegates to the shared one too. This resolves the
  ADR "Open Implementation Questions": the shared seam owns header projection +
  request/publish + timeouts, while the **typed decode** stays in the domain
  crate (ACP decodes the generic `Message`; MCP decodes rmcp's
  `RxJsonRpcMessage`). MCP keeps a thin `rmcp::Transport` (Sink/Stream) adapter
  over those shared primitives because rmcp fixes its transport shape and its
  server-initiated streaming + JetStream keepalive ack are domain concerns that
  do not belong in the shared core.
- **Edges**: canonical JSON-RPC is reconstructed at the protocol edges and the
  `jsonrpc == "2.0"` invariant is enforced there. Batch unbundling is not
  applicable to our edges — the A2A HTTP edge is one envelope per call and the
  MCP HTTP edge's batching is handled by the rmcp SDK — so no backbone message
  ever carries a JSON-RPC array.

The JSON-RPC over NATS binding (ADR 0011) is complete: ACP, MCP, and A2A all run
on the shared content-mode codec and transport, with the invariants enforced and
the round-trip property test gating the codec.

## Deferred (later, non-blocking)

- **Centralize edge reconstruction through the codec.** Edges today rebuild
  canonical JSON-RPC with local `serde_json` builders rather than the codec's
  `to_json_value`/`from_json_value` (currently exercised only in tests). Behavior
  is already correct and the invariant holds; routing every edge through the
  shared reconstruction is a consistency cleanup to fold into the broader
  refactor, not part of closing out this binding.

## Out of Scope (not JSON-RPC binding work)

- **Per-message A2A signing.** JSON-RPC defines no signing; message signing is an
  A2A *security* concern that belongs to A2A's own design, not this binding. It
  is also not implemented today — A2A does not sign request/response traffic
  (the only signing in the repo is the offline skill-*bundle* signer in
  `a2a-redaction/src/signed_bundle/`). The only carryover this binding owes that
  future work is a one-line constraint: because content mode moved `id` and
  `error.code` into the `Jsonrpc-Id`/`Jsonrpc-Error-Code` headers, any future
  per-message signing must cover those headers, not just the body. The guardrail
  for that already lives in code — `a2a-nats/src/wire.rs` keeps signed A2A paths
  disabled until such a scheme exists — so nothing here is blocked on it.

## Invariants to Enforce (ADR "Invariants")
- `decode(encode(m)) == m` for every valid message, id type included.
- `jsonrpc` is constant `"2.0"`, re-injected on decode; an edge rejects any
  reconstructed envelope whose `jsonrpc` is absent or not `"2.0"` (hard fail,
  `CodecError::UnsupportedVersion`) rather than coercing it.
- Response is an error iff `Jsonrpc-Error-Code` (integer) is present; error has a
  `message` body, success has a `result` body.
- Response with neither `result` body nor `Jsonrpc-Error-Code` = protocol error.
- `Jsonrpc-Id` is the id's JSON literal; absent = notification (request) or
  `null` (response), told apart by direction. Requests use a non-null id.
- Method carried by the subject.
- The JSON-RPC `id` is **not** the transport correlation key; correlation
  (`X-Req-Id`, reply inbox, JetStream response consumer) stays a transport
  concern outside the codec.

## Risks & Open Questions
1. **Header-dependent interpretation.** The body alone is no longer
   interpretable; consumers must read headers. Verify every consumer (error
   counters, dead-letter routers, id indexes) reads headers and that nothing
   replays raw bodies.
2. **Per-subject cutover discipline.** A subject must never carry both encodings
   simultaneously. Sequence any remaining migrations and gate each on tests.

## Validation
- `mise exec -- cargo test` per crate, plus the dylint module-policy lints.
- Testcontainers-backed integration tests for real NATS paths (ADR 0010) on the
  migrated subjects.
- Per-handler regression tests proving structured errors survive (the
  `authenticate` case is the canonical one).

## References
- [ADR 0011: JSON-RPC over NATS Binding](./docs/adr/0011-jsonrpc-over-nats-binding.md)
- [ADR 0003: AI Protocol Transport Taxonomy](./docs/adr/0003-ai-protocol-transport-taxonomy.md)
- [ADR 0004: Protocol and Transport Layering](./docs/adr/0004-protocol-and-transport-layering.md)
- [ADR 0009: Protocol Buffers Wire Contracts](./docs/adr/0009-protocol-buffers-wire-contracts.md)

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

## Remaining Work

### Phase 6 (signing only) — Extend per-message signing to cover the headers
⚠️ **Blocked.** ADR §5 says to extend A2A signing so it spans `Jsonrpc-Id` and
`Jsonrpc-Error-Code` in addition to the body. But there is **no per-message A2A
request/response signing scheme in the repo today** — only the offline Ed25519
*bundle* signer (`a2a-redaction/src/signed_bundle/`). Until that scheme is
located or defined, signed A2A paths must stay disabled (see the note in
`a2a-nats/src/wire.rs`). First step is a decision: is per-message signing in
design, out of repo, or to be specified here?

### Phase 7 — Edges and batch
- Audit every protocol edge (HTTP/WebSocket/SSE listeners, stdio bridges) so
  canonical JSON-RPC is reconstructed **only** there, centralized in the codec,
  never ad hoc in domain code. `acp-nats-server` already reconstructs + unbundles
  via `IncomingHttpMessage::parse_all`; confirm/extend the same discipline for
  the A2A (`a2a-nats-http`, `a2a-nats-stdio`) and MCP edges.
- Unbundle batch JSON-RPC arrays at the edge into individual messages so the
  backbone carries one JSON-RPC message per NATS message (Design Rules).

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
1. **A2A per-message signing may not exist yet** (gates Phase 6 signing). Only
   offline bundle signing was found; confirm whether a per-message scheme is in
   design or out of repo, and keep the encoding off signed paths until it covers
   the headers.
2. **Header-dependent interpretation.** The body alone is no longer
   interpretable; consumers must read headers. Verify every consumer (error
   counters, dead-letter routers, id indexes) reads headers and that nothing
   replays raw bodies.
3. **Per-subject cutover discipline.** A subject must never carry both encodings
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

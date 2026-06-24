# Plan: Adopt ADR 0011 — JSON-RPC over NATS Binding

Implements [ADR 0011](./docs/adr/0011-jsonrpc-over-nats-binding.md) across the
Rust workspace. The goal is one shared, protocol-agnostic codec that binds any
JSON-RPC 2.0 protocol (ACP, MCP, A2A) onto NATS in **binary content mode**:
control/correlation fields project to the subject and `Jsonrpc-*` headers, the
payload stays in the body, and success-versus-error is decided by the presence
of the `Jsonrpc-Error-Code` header — never by structural deserialization.

## Current State (verified)

| Protocol | Today's NATS mapping | Gap vs. ADR 0011 |
| --- | --- | --- |
| **ACP command path** (client→agent) | Bare params in body, method = subject, correlation in `X-Req-Id`, bare result in reply | No `Jsonrpc-Error-Code`/`Jsonrpc-Id` headers; success/error told apart by deserialize-and-fallback (only on JetStream), not on core req/reply |
| **ACP callback path** (agent→client) | Full JSON-RPC envelope in body | Inconsistent with command path; envelope not projected to headers |
| **MCP** | Full JSON-RPC envelope in body, end to end | No header projection; codec is private free fns, not shared |
| **A2A** | Raw JSON-RPC body; `id` and `error.code` live only in body | No `Jsonrpc-*` headers; routes by subject but ignores body `method` |

Key code anchors:

- MCP codec to generalize: `rsworkspace/crates/mcp-nats/src/transport.rs:260-268`
  (`serialize_message` / `deserialize_message`, private free fns inside
  `NatsTransport<R, N>`).
- ACP error-loss bug: `rsworkspace/crates/acp-nats/src/agent/authenticate.rs`
  + `rsworkspace/crates/acp-nats/src/error.rs:31-33` — a structured agent
  `Error` fails to deserialize as `AuthenticateResponse` and collapses into
  `InternalError`. The JetStream path
  (`rsworkspace/crates/acp-nats/src/agent/js_request.rs:72-85`) has a two-pass
  fallback; the core request/reply path does not. The codec replaces both with
  the header discriminator.
- ACP envelope split: command path strips the envelope; callback path keeps it
  (`rsworkspace/crates/acp-nats/src/client/rpc_reply.rs`,
  `.../client/request_permission.rs`,
  `rsworkspace/crates/acp-nats/src/jsonrpc.rs`).
- Shared NATS primitives: `rsworkspace/crates/trogon-nats/src/` (client traits,
  `messaging.rs`, `jetstream/`, `constants.rs:REQ_ID_HEADER`). No `jsonrpc`
  module exists yet; no `Jsonrpc-*` header exists anywhere.
- A2A wire: `rsworkspace/crates/a2a-nats/src/{client,server}/wire.rs`,
  `.../jsonrpc.rs` (`extract_request_id`), `.../server/dispatch.rs`
  (`A2aMethod::from_subject`).
- Redaction stays in the body: `rsworkspace/crates/a2a-redaction/` operates on
  `Message.parts` / `Artifact.parts` — unaffected by header projection.

## Target Architecture

A new crate **`jsonrpc-nats`** (`rsworkspace/crates/jsonrpc-nats/`), per the
`<protocol>-<backbone>` convention in ADR 0003 and the package working name in
ADR 0011 §6. It depends on `trogon-nats` client traits and owns:

- The field mapping and its inverse (`encode` / `decode`).
- `Jsonrpc-Id` JSON-literal encoding (ASCII-escaped) with type preservation.
- Success-vs-error discrimination via `Jsonrpc-Error-Code` presence.
- Notification semantics (absent `Jsonrpc-Id`, disambiguated by direction).
- Transport-failure → JSON-RPC error mapping.
- Edge reconstruction to/from canonical JSON-RPC.

Domain crates (`acp-nats`, `mcp-nats`, A2A) inject only what is domain-specific:
subject routing (method+context→subject), transport selection (core
request/reply vs. JetStream durable), and typed params/results.

### Header contract (ADR §1, §3)

| JSON-RPC field | NATS location | Rule |
| --- | --- | --- |
| `jsonrpc` | — | constant `"2.0"`, re-injected on decode |
| `method` | subject | routed per existing subject schemes |
| `id` | `Jsonrpc-Id` | JSON literal (`42`, `"42"`, `"abc"`); absent = notification (request) or `null` (response) |
| `params` / `result` | body | result present iff no `Jsonrpc-Error-Code` |
| `error.code` | `Jsonrpc-Error-Code` | integer; presence = the discriminator |
| `error.message` / `error.data` | body | — |

## Phases

Order follows the ADR Migration section. Cut over **per handler and per
subject**; a subject never carries both the old and new encoding at once.

### Phase 0 — Codec crate scaffold ✅
- Create `rsworkspace/crates/jsonrpc-nats/` (wire into `rsworkspace/Cargo.toml`
  workspace members; follow ADR 0002 crate boundaries and ADR 0005 layout).
- Define the codec API: `encode(message) -> (subject-input, HeaderMap, Bytes)`
  and `decode(direction, HeaderMap, Bytes) -> message`, plus the `Jsonrpc-Id`
  literal codec and the `Jsonrpc-Error-Code` discriminator. Direction
  (request vs. response) is an explicit input, since it disambiguates an absent
  `Jsonrpc-Id` (notification vs. `null`).
- Add header-name constants `Jsonrpc-Id`, `Jsonrpc-Error-Code` (these are the
  only two `Jsonrpc-*` headers; do **not** put correlation under `Jsonrpc-*`).

### Phase 1 — Round-trip invariant (gates everything downstream) ✅
- Property test + fuzz: `decode(encode(m)) == m` for every valid message,
  **id type included** — numbers, strings, numeric-looking strings, large
  integers, `null`, unicode string ids, results, errors, notifications
  (ADR §4, Invariants). No such test exists today; add `proptest` to the crate.
- Assert the discriminator invariants: error iff `Jsonrpc-Error-Code` present;
  response with neither `result` body nor error code is a protocol error;
  numeric `1` and string `"1"` stay distinct on the wire.

### Phase 2 — Prove on MCP (generalize the existing transport) ✅
- Replace the private `serialize_message`/`deserialize_message` in
  `mcp-nats/src/transport.rs` with the shared codec; move MCP from
  full-envelope-in-body to content-mode encoding.
- Keep MCP subject routing (`method_suffix`, `nats/subjects/`,
  `nats/parsing.rs`) in `mcp-nats` as the injected domain piece.
- Reconstruct canonical JSON-RPC at the MCP edges only — stdio bridge
  (`mcp-nats-stdio`) and Streamable HTTP/SSE (`mcp-nats-server/src/runtime.rs`,
  `McpNatsProxyService`).
- Update existing transport tests; the body is no longer self-interpreting, so
  tests must read headers to interpret it.

### Phase 3 — Prove on ACP `authenticate` end to end (the error-loss bug) ✅
- Route `authenticate` through the codec: agent emits `Jsonrpc-Error-Code` on
  failure; the bridge decides success/error from the header, not from a failed
  deserialize. This eliminates the `InternalError` collapse in
  `acp-nats/src/error.rs:31-33` and restores the original code/message —
  auth rejection becomes distinguishable from an unavailable agent.
- Touch points: `acp-nats/src/agent/authenticate.rs`,
  `acp-nats/src/error.rs`, runner reply path in
  `acp-nats-agent/src/connection.rs` (`handle_request`).
- Add a regression test: agent returns a structured auth error → bridge surfaces
  the original code/message (not `-32603`).

### Phase 4 — Roll remaining ACP handlers onto the codec 🚧

**Status:** Global core req/reply handlers (`initialize`, `authenticate`, `logout`,
`session/new`, `session/list`, `ext.*`) and JetStream session commands except
`prompt` (`load`, `set_mode`, `set_config_option`, `set_model`, `fork`,
`resume`, `close`) use the codec on the bridge and agent runner. `js_request`
uses header-discriminated responses. Agent-runner and bridge unit tests updated
for wire encoding. **Remaining:** `prompt` JetStream path (`handle_js` in
`prompt.rs`), callback path (agent→client), `cancel` notification encoding.

### Phase 5 — Fold `mcp-nats` and `acp-nats` onto one shared transport ⏳
- Once both run on the codec, converge them onto a single transport + wire
  mapping so ACP and MCP share one implementation (ADR Consequences).
- Resolve the **open implementation questions** (ADR §"Open Implementation
  Questions"): the transport trait spanning core request/reply and JetStream
  durable delivery; and how ACP server-initiated streaming (prompt notification
  stream + final response) and JetStream keepalive ack compose on top of the
  peer primitives without entering the shared core.

### Phase 6 — A2A: project headers + extend signing ⏳
- Promote `id` → `Jsonrpc-Id` and `error.code` → `Jsonrpc-Error-Code` in
  `a2a-nats/src/{client,server}/wire.rs`; keep subject-based dispatch
  (`A2aMethod::from_subject`). Route A2A through the shared codec.
- **Extend signing to cover the authoritative headers** (ADR §5, Design Rules).
  ⚠️ Open question — see Risks: today only an offline Ed25519 *bundle* signing
  scheme exists (`a2a-redaction/src/signed_bundle/`); there is no per-message
  A2A request/response signing yet. The per-message signing scheme that ADR §5
  amends must be located or defined before headers can be folded into its
  signing input. Signing must span `Jsonrpc-Error-Code` and `Jsonrpc-Id` in
  addition to the body. **Do this before the encoding is used on any signed
  path.**
- Confirm redaction is unaffected: `result`, `message`, `data`, `params` stay
  in the body where the redaction pipeline operates.

### Phase 7 — Edges and batch ⏳
- Audit every protocol edge (HTTP/WebSocket/SSE listeners, stdio bridges) to
  ensure canonical JSON-RPC is reconstructed **only** there and centralized in
  the codec — never ad hoc in domain code.
- Unbundle batch requests at the edge into individual messages; the backbone
  carries one JSON-RPC message per NATS message (Design Rules).

## Invariants to Enforce (ADR "Invariants")
- `decode(encode(m)) == m` for every valid message, id type included.
- `jsonrpc` is constant `"2.0"`, re-injected on decode.
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
1. **A2A per-message signing may not exist yet.** ADR §5 says "extend the A2A
   signing scheme," but only offline bundle signing was found. Confirm whether a
   per-message signing scheme is in design or out of repo before Phase 6;
   blocking the encoding on signed paths until it covers the headers.
2. **Header-dependent interpretation.** The body alone is no longer
   interpretable; consumers must read headers. Acceptable because JetStream
   persists headers — but verify every consumer (error counters, dead-letter
   routers, id indexes) reads headers, and that nothing replays raw bodies.
3. **Hot-path codec correctness.** The codec is core infrastructure on the hot
   path; its correctness rests entirely on the Phase 1 round-trip property test.
   That test gates merge of every later phase.
4. **`Jsonrpc-Id` subject-safety / control chars.** Arbitrary string ids are not
   subject-safe (hence id stays in a header, method stays in subject); ASCII-
   escape the literal so values are free of raw control characters (NATS forbids
   only CR/LF in header values).
5. **Per-subject cutover discipline.** A subject must never carry both encodings
   simultaneously. Sequence handler/subject migrations and gate each on tests.

## Validation
- `mise exec -- cargo test` per crate as each phase lands, plus the dylint
  module-policy lints (recent workspace policy).
- Testcontainers-backed integration tests for real NATS paths (ADR 0010) on the
  migrated subjects.
- Per-handler regression tests proving structured errors survive (the
  `authenticate` case is the canonical one).

## References
- [ADR 0011: JSON-RPC over NATS Binding](./docs/adr/0011-jsonrpc-over-nats-binding.md)
- [ADR 0003: AI Protocol Transport Taxonomy](./docs/adr/0003-ai-protocol-transport-taxonomy.md)
- [ADR 0004: Protocol and Transport Layering](./docs/adr/0004-protocol-and-transport-layering.md)
- [ADR 0009: Protocol Buffers Wire Contracts](./docs/adr/0009-protocol-buffers-wire-contracts.md)

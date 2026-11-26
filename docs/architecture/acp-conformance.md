# ACP Conformance

This document is the single source of truth for where this repository stands relative to the Agent Client Protocol (ACP) specification. Update it in the same PR as any `agent-client-protocol` version bump or any change to the bridged method surface.

## Spec position

| Fact | Value |
| --- | --- |
| Wire protocol | v1 |
| Pinned Rust SDK | `agent-client-protocol` 5.0.0 (`rsworkspace/Cargo.toml`) |
| Bundled schema (effective spec level) | 1.15.1; the direct schema dependency matches the SDK pin |
| Latest upstream SDK at last review | 5.0.0 (2026-11-02) |
| Latest upstream schema at last review | 1.16.0 (2026-11-09) |
| Highest adoptable schema | 1.15.1; the SDK pins `agent-client-protocol-schema` at exactly `=1.15.1` |
| Last reviewed | 2026-11-09 |

Upstream repositories: [spec/schema](https://github.com/agentclientprotocol/agent-client-protocol), [Rust SDK](https://github.com/agentclientprotocol/rust-sdk).

The SDK's exact schema pin caps our effective spec level. The direct schema dependency must match that pin so Cargo feature unification applies to the same types. SDK 5.0.0 forwards every v1 opt-in flag used here; proxy-chain schema types are enabled by the SDK itself. Pinning it past the SDK's requirement would put two schema crates in the graph, at which point the flags no longer apply to the types the SDK actually re-exports, so schema releases above the SDK's pin are tracked as gaps here rather than adopted early.

## Companion crates

The Rust SDK repository publishes more than the core crate, so tracking only `agent-client-protocol` hides capability that upstream already ships. The freshness task reads the recorded versions straight out of this table, so it stays the one place they live.

Most companions release in lockstep with the core crate: `-http`, `-conductor`, `-polyfill`, `-derive`, and `-trace-viewer` have carried the core's exact version number on the same day since 1.0.0, so a core bump is a reliable signal that they moved too. The schema lives in the spec repository and versions independently (the SDK pins it exactly, see above). `-rmcp` is now aligned at 5.0.0 but has an independent history, and `-tokio` has not released since 2026-04-21. The cookbook is also published; the upstream test harness and `yopo` workspace package were not on crates.io at this review.

| Crate | Latest | Adopted | Position |
| --- | --- | --- | --- |
| `agent-client-protocol` | 5.0.0 | yes | The core dependency this document tracks. |
| `agent-client-protocol-schema` | 1.16.0 | yes (1.15.1) | Capped by the SDK's exact pin, see above. |
| `agent-client-protocol-derive` | 5.0.0 | transitive | Pulled in by the core crate; no direct dependency. |
| `agent-client-protocol-http` | 5.0.0 | yes (`server` feature) | Official axum-based Streamable HTTP + WebSocket transport. It now serves all remote traffic: `main` builds `AcpHttpServer` over a per-connection `NatsAgentComponent` (`acp-nats-server/src/component.rs`), which retired the hand-rolled `transport.rs` and `connection.rs` (3,052 lines with their tests). `compat.rs` layers back the three behaviors upstream omits: `Origin` enforcement on every verb (upstream checks it server-side only on the WebSocket upgrade), `Acp-Protocol-Version` validation against the negotiated version (the transport spec says clients SHOULD send it and upstream never reads it), and `X-Accel-Buffering: no` on SSE. Drain is handled inside the component, which selects on a shutdown watch. Client feature deliberately not enabled: it wants reqwest 0.13 against the workspace's `=0.12.28`. See "Remote transport behavior changes" below. |
| `agent-client-protocol-polyfill` | 5.0.0 | no | Re-evaluated at 5.0.0: a conductor-chain proxy exposing request-scoped native MCP servers through loopback HTTP to agents supporting MCP 2026-07-28. This bridge does not host a conductor chain or route native MCP requests, so adopting the polyfill does not close its routing gap. |
| `agent-client-protocol-rmcp` | 5.0.0 | no | Re-evaluated at 5.0.0: the default feature set contains no rmcp integration; `rmcp_3` opts into rmcp 3.4.0 server adapters. The workspace already uses rmcp 3.x, but the ACP bridge does not host native SDK MCP servers, so this adapter has no runtime role here. |
| `agent-client-protocol-conductor` | 5.0.0 | no | Re-evaluated at 5.0.0: binary and library for stdio proxy chains using `proxy/initialize` and `proxy/successor`. Chain construction and successor handoff remain a different problem from the subject-routed NATS bridge. |
| `agent-client-protocol-tokio` | 0.11.1 | no | Not updated since 2026-04-21 and superseded by the core crate's own runtime-agnostic surface. Treated as dormant; a release would be worth investigating. |
| `agent-client-protocol-trace-viewer` | 5.0.0 | no | Re-evaluated at 5.0.0: protocol trace inspection remains developer tooling, with no runtime role in the bridge. |
| `agent-client-protocol-cookbook` | 5.0.0 | no | Published examples and recipes, re-evaluated at 5.0.0. Reference material rather than a bridge runtime dependency. |

### Remote transport behavior changes

Adopting the official transport moved several decisions from this repository to upstream. None is a spec regression, and two are strictly more correct, but each is observable to a client.

| Change | Before | Now | Why it is acceptable |
| --- | --- | --- | --- |
| `connectionId` in the `initialize` result body | injected | absent | Not a field in the ACP v1 schema. Upstream returns it only as the `Acp-Connection-Id` header, which is the conformant surface. |
| Connection id format | UUIDv7 (time-sortable) via `AcpConnectionId` | UUIDv4 | `ConnectionRegistry::next_connection_id` is private and hardcoded, so the value object no longer governs the wire and was deleted. This is a genuine loss, not an improvement. |
| SSE stream scoping | every message broadcast to all listeners on a connection | routed by whether the payload carries `sessionId` | Upstream splits connection-scoped and session-scoped streams. A `session/update` always carries a session id, so it reaches that session's listeners rather than everyone on the connection. Less leakage between sessions. |
| Session-scoped request missing `Acp-Session-Id` | 400 | 202, routed from `params.sessionId` | Upstream recovers the session from the payload instead of demanding the header. |
| Request naming an unknown session | 404 | 202, forwarded | Session lifetime belongs to the agent, not the transport. The old transport kept its own set of known sessions to answer 404 itself. |
| Connection id after a *failed* `initialize` | returned | not returned | Upstream tears the half-built connection down. The old behavior handed out an id for a connection that never initialized. |
| Browser WebSocket upgrade | allowed for same-origin/loopback | rejected | `ServerOptions::default` uses `CorsOptions::Disabled`, which rejects any request carrying an `Origin` header on the upgrade path. Non-browser clients send no `Origin` and are unaffected. Recoverable in one line (`CorsOptions::AllowAnyOrigin`) if a browser client appears, since `compat::enforce_origin` is the real gate either way. |
| `/health` | absent | served | `ServerOptions::default` provides it and nothing in the repo conflicts. |
| JSON-RPC arrays (SDK 5.0 migration) | accepted as batches | rejected without dispatching entries | Upstream removed batch handling from every SDK transport. HTTP returns 400 with `-32600` and `id: null`; WebSocket and stdio return the same JSON-RPC error. This follows the [remote transport RFD](https://github.com/agentclientprotocol/agent-client-protocol/pull/2729) and [SDK change](https://github.com/agentclientprotocol/rust-sdk/pull/454). |
| `initialize.protocolVersion` (SDK 5.0 migration) | backend response could retain legacy version 0 | the explicit v1 connection returns version 1 | SDK version negotiation selects the connection protocol and normalizes the response. Subsequent `Acp-Protocol-Version` headers must match the returned version; the compatibility layer still rejects mismatches. |
| Session SSE stream (SDK 5.0 migration) | unbounded stream allocation for unknown sessions | bounded pre-load streams plus agent-confirmed sessions | Upstream permits limited unconfirmed streams so clients can subscribe before `session/load` replay. Agent responses confirm sessions; unknown IDs return 404 once those unconfirmed slots are full. Caller headers alone do not establish known sessions. |
| Concurrent SSE readers (SDK 5.0 migration) | messages broadcast to multiple readers of one logical stream | one active reader per connection or session stream | A duplicate reader receives 409. Different logical streams remain independent, and the accepted reader receives queued responses and updates. |

## Policy

Opt in to unstable spec features ahead of stabilization. The default for every unstable feature is to enable the flag, wire the routing, and test it. Opting out is the exception and requires a rationale in the matrix below.

## Why this matters here

The [bridge](../glossary/bridge) decodes every message into typed SDK structs and re-serializes them (`acp-nats/src/wire.rs`). Fields the pinned SDK does not model are silently stripped in transit, and unknown `session/update` variants fail decode. Spec lag means silent data loss, not graceful passthrough, so this matrix must stay accurate.

## Conformance matrix

Status values: `implemented` (routed, typed, tested), `capabilities implemented` (capability payloads round-trip, but the methods behind them are not routed), `half-wired` (one half of the path is routed and tested, but the end-to-end flow cannot complete in a release build, so the surface does not work in production), `unwired` (SDK flag enabled but no routing), `dropped` (peers may send it, the bridge strips or rejects it), `unrepresentable` (pinned SDK cannot express it), `not supported` (deliberate opt-out with rationale), `watch-only` (tracked for adoption, deliberately not implemented while upstream still churns).

A green test suite does not earn a status of `implemented`. `half-wired` exists because a surface can be fully unit-tested on both sides of a seam that nothing connects in a release build.

### Agent-side methods (client to agent)

| Spec surface | Spec stage (schema 1.15.1) | Our status | Notes |
| --- | --- | --- | --- |
| `initialize` | stable | implemented | |
| `authenticate` | stable | implemented | terminal authentication shapes are stable |
| `logout` | stable (0.13.3) | implemented | |
| `session/new` | stable | implemented | includes `additionalDirectories` |
| `session/load` | stable | implemented | includes `additionalDirectories` |
| `session/list` | stable | implemented | |
| `providers/list` | unstable (0.11.7) | implemented | `unstable_llm_providers` now reaches the SDK request table; boundary dispatch forwards to the existing bridge and NATS handler. |
| `providers/set` | unstable (0.11.7) | implemented | Routed through the SDK boundary and existing NATS handler with `unstable_llm_providers`. |
| `providers/disable` | unstable (0.11.7) | implemented | Routed through the SDK boundary and existing NATS handler with `unstable_llm_providers`. |
| `session/prompt` | stable | implemented | |
| `session/cancel` (notification) | stable | implemented | |
| `session/set_mode` | stable | implemented | |
| `session/set_config_option` | stable | implemented | 1.4.0 shape, boolean and `model_config` round-trip tested |
| `session/set_model` | **removed upstream** (0.13.5) | removed | deleted with the SDK migration; model switching goes through `model_config` config options |
| `session/fork` | unstable | implemented | |
| `session/resume` | stable (0.12.2) | implemented | |
| `session/close` | stable (0.12.2) | implemented | |
| `session/delete` | stable (0.13.6) | implemented | routed end to end with tests, span `acp.session.delete` |
| JSON-RPC request cancellation | stable (1.2.0) | implemented | boundary honors `$/cancel_request`: bridge-side work is dropped and the request answers with `request_cancelled` (tested); prompt-turn cancellation on the runner side remains `session/cancel` per spec |
| JSON-RPC batches (inbound) | removed from SDK 5.0.0 | not supported | Arrays are rejected with one invalid-request error and no entry dispatch. See the transport behavior changes above. |
| `auth/status` | unstable (1.15.0) | implemented | Global NATS subject `auth.status` and boundary handler. |
| `session/status` | unstable (1.15.0) | implemented | Session-scoped status request. |
| `session/set_title` | unstable (1.15.0) | implemented | Session-scoped title update. |
| `session/archive`, `session/unarchive` | unstable (1.15.0) | implemented | Session-scoped archive operations; list filtering and returned archive metadata also survive typed transport. |
| `proxy/initialize`, `proxy/successor` | unstable (1.15.0) | not supported | These install proxy-chain topology and successor handoff. The bridge is an Agent endpoint with per-connection NATS dispatch, not an SDK proxy endpoint; forwarding the requests alone cannot establish a chain. |
| `ext/*` (extension methods) | stable | implemented | passthrough |

### Client-side methods (agent to client)

| Spec surface | Spec stage | Our status | Notes |
| --- | --- | --- | --- |
| `fs/read_text_file` | stable | implemented | |
| `fs/write_text_file` | stable | implemented | |
| `session/request_permission` | stable | implemented | |
| `session/update` | stable | implemented | unknown variants fail decode and are dropped with a `session_update`/`decode_failure` error metric |
| `terminal/create` | stable | implemented | |
| `terminal/output` | stable | implemented | |
| `terminal/release` | stable | implemented | |
| `terminal/wait_for_exit` | stable | implemented | |
| `terminal/kill` | stable | implemented | |
| `elicitation/create` | stable (1.7.0) | implemented | routed both through the bridge-owned `ClientHandler::elicitation_create` and the SDK byte-stream boundary (`AgentRequest::CreateElicitationRequest`); `ElicitationScope::Request` (pre-session, no session id) is not routable since all NATS client subjects and `NatsClientProxy` construction are session-scoped |
| `elicitation/complete` (notification) | stable (1.7.0) | implemented | routed as a `ClientHandler::elicitation_complete` notification |
| `mcp/message` (request and notification) | unstable (1.10.0 request-scoped revision) | not supported | Native MCP messages bind to a containing request, not a session. The bridge has no request-lifetime MCP host or client destination; preserving the payload alone cannot route it. |
| `mcp/connect`, `mcp/disconnect` | removed (1.10.0) | removed | Replaced upstream by request-scoped `mcp/message`; the bridge never routed the earlier connection-scoped lifecycle. |
| `auth/status_update` | unstable (1.15.0) | not supported | This notification can precede any session. Client subjects and `NatsClientProxy` construction require a session ID, so the bridge lacks a connection-scoped client destination. Do not advertise working push status through this bridge. |
| `ext/*` | stable | implemented | passthrough |
| `ext/session/prompt_response` (bridge extension, not spec) | n/a | removed | Reverses this row's earlier "deliberately retained" position, on evidence that was not in front of it. The extension was the completion half of the original prompt design (#31): register a session waiter, publish the prompt, and take the answer back as a client-role notification correlated on `meta.prompt_id`. #43 replaced that whole design with request/reply, and the current path uses a JetStream response consumer filtered on `{req_id}`, which is durable across a restart in a way a process-local `oneshot` map is not. What survived #43 was the receive half alone, with `register_waiter` marked `#[cfg(test)]`, so `resolve_waiter` always returned `false` and every notification was logged and dropped. Completing it was not possible either: nothing in the workspace publishes this method, and no subject type exists to publish it with, so the send half had no counterpart to build against. The retained rationale (a response outliving its caller) is real but belongs to the [ADR#0055](../adr/0055-nats-subject-design-jsonrpc-bindings.md) `Jsonrpc-Id` demux, which needs many concurrent waiters per session; this registry allowed exactly one and rejected a second. Now deleted: the registry, its suppression window, the handler, the `ClientMethod` variant, and the orphaned semconv entries. The subject still parses, as a generic `ext.` method, so a publisher that sends it reaches the client's ext handler instead of a decode-and-drop. |

### Payload-level capabilities

| Spec surface | Spec stage | Our status | Notes |
| --- | --- | --- | --- |
| `additionalDirectories` (session/new, session/load) | stable (0.13.5) | implemented | round-trip tested through the bridge |
| Message IDs on chunks | stable (0.13.6) | implemented | 1.4.0 shape, round-trip tested |
| Session usage updates | stable (0.13.6) | implemented | 1.4.0 shape, round-trip tested |
| Session config options | stable | implemented | 1.4.0 shape, `ConfigOptionUpdate` round-trip tested |
| Boolean config options | stable (1.3.0) | implemented | stabilized shape, round-trip tested |
| `model_config` option category | stable (1.1.0) | implemented | round-trip tested |
| NES (next edit suggestions) | removed (1.12.0) | removed | Upstream removed the types and flag. Existing elicitation capability coverage remains; there is no NES payload to preserve in schema 1.15.1. |
| Plan operations | unstable (0.13.4) | implemented | `PlanUpdate`/`PlanRemoved` round-trip tested with `unstable_plan_operations` |
| Providers | unstable (0.11.7) | implemented | Capability payloads and `providers/list`, `providers/set`, `providers/disable` are representable and routed with `unstable_llm_providers`. |
| MCP-over-ACP message types | unstable (1.10.0 request-scoped revision) | capabilities implemented | `McpServer::Acp`, capability payloads, and request-scoped `requestId`/`serverId` message envelopes survive typed transport. `mcp/message` requests and notifications remain deliberately unrouted: the bridge has no request-lifetime MCP binding or native MCP server host. Schema 1.10 removed `mcp/connect` and `mcp/disconnect` together with connection IDs. Enabling the flag alone does not implement that lifecycle. |
| Elicitation enum option descriptions | stable (1.7.0) | implemented | `EnumOption` descriptions on `StringPropertySchema.one_of` round-trip tested |
| Tool call `name` | stable (1.8.0) | implemented | Nonempty names on `tool_call` and `tool_call_update` survive public wire decode and re-encode. |
| Session notices and compaction | stable (1.11.0) | implemented | Client `session.notices`/`session.compaction` capabilities and `notice`, `compaction_update`, `compaction_summary_chunk` updates round-trip. |
| Subagents and session message views | unstable (1.10.0, revised 1.13.0/1.14.0) | implemented | `unstable_subagents` preserves subagent identity, working directories, message references, and the `subagent_update`, `session_message`, `session_message_chunk` variants. |
| Client system prompt | unstable (1.12.0) | implemented | `unstable_session_system_prompt` preserves `systemPrompt` on new/load/fork/resume and its capability. Stabilized in schema 1.16.0, which remains above the SDK exact pin. |
| Archive and status metadata | unstable (1.15.0) | implemented | Archive list filters/metadata and auth/session lifecycle capabilities round-trip. |
| Protocol v2 | unstable, heavy churn | watch-only | the SDK still gates it behind `unstable_protocol_v2`; the freshness workflow surfaces releases, and this migration keeps the existing v1-only boundary |

## SDK 5.0 review

The [schema changelog](https://github.com/agentclientprotocol/agent-client-protocol/blob/v1.15.1/CHANGELOG.md) was reviewed from 1.5.0 through 1.15.1 alongside the [SDK changelog](https://github.com/agentclientprotocol/rust-sdk/blob/v5.0.0/src/agent-client-protocol/CHANGELOG.md). Schema 1.6 added tool names; 1.7 stabilized elicitation and terminal authentication and introduced compaction; 1.8/1.9 added notices and stabilized tool names; 1.10 introduced subagents and request-scoped MCP; 1.11 stabilized notices/compaction; 1.12 removed NES and added client system prompts; 1.13/1.14 revised subagent message identity and working directories; 1.15 added status, title, archive, and proxy methods. Patch releases also tightened decoding and clarified protocol behavior.

SDK-boundary adapters explicitly select `Agent.v1()` / `Client.v1()`, use `agent_client_protocol::v1::schema`, and retain the bridge-owned callback traits. Public regression coverage lives in `acp-nats/tests/schema_1_15_conformance.rs`, `acp-nats/tests/method_routing.rs`, and `acp-nats/tests/schema_1_15_dispatch.rs`; boundary and HTTP tests exercise dispatch, cancellation, shutdown, and array rejection. These tests establish bridge behavior, not support for the deliberately excluded proxy and MCP lifecycles.

## Upgrade ritual

A version bump of `agent-client-protocol` (or the schema it bundles) is never just a version change. Every bump PR must:

1. Diff the schema changelog between the old and new pinned versions ([changelog](https://github.com/agentclientprotocol/agent-client-protocol/blob/main/CHANGELOG.md)).
2. For each added or stabilized method: add subject mapping in `acp-nats/src/nats/parsing.rs`, a handler (bridge trait method plus its match arm in the boundary dispatch and [NATS](../glossary/nats) dispatch), and tests, or add a matrix row with an opt-out rationale. The byte-stream boundary no longer needs per-method registrations: `connect_agent_boundary` routes through the SDK's `ClientRequest`/`ClientNotification` enums, so a method missing from its match answers `method_not_found` instead of being silently unreachable (see [ADR#0020](../adr/0020-acp-sdk-1x-boundary-and-bridge-traits.md), amendment of 2026-07-09).
3. For each added field or `session/update` variant: add a round-trip test through the bridge. Typed re-encode means unmapped fields are silently dropped, so a green compile proves nothing about coverage.
4. For each new unstable flag: enable it per the opt-in policy and wire it. A flag that only exists in a schema release above the SDK's exact pin cannot be enabled; move the direct schema dependency in lockstep with the SDK's requirement and record the gap as a matrix row instead.
5. Update the `## Companion crates` table with the new versions, and re-evaluate every row marked `no`. Companions release in lockstep with the core crate, so a bump is exactly when a gap that blocked adoption may have closed. The freshness task parses that table, so leaving it stale disables the check.
6. Update this document (matrix and spec position table) in the same PR.

The scheduled freshness workflow (`.github/workflows/acp-freshness.yml`) embeds this checklist in the issue it files when drift is detected.

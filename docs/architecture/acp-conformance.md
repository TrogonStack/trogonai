# ACP Conformance

This document is the single source of truth for where this repository stands relative to the Agent Client Protocol (ACP) specification. Update it in the same PR as any `agent-client-protocol` version bump or any change to the bridged method surface.

## Spec position

| Fact | Value |
| --- | --- |
| Wire protocol | v1 |
| Pinned Rust SDK | `agent-client-protocol` 2.0.0 (`rsworkspace/Cargo.toml`) |
| Bundled schema (effective spec level) | 1.5.0 (plus direct `agent-client-protocol-schema` dependency for schema-only unstable flags) |
| Latest upstream SDK at last review | 2.0.0 (2026-07-23) |
| Latest upstream schema at last review | 1.6.0 (2026-07-21) |
| Highest adoptable schema | 1.5.0 — the SDK pins `agent-client-protocol-schema` at exactly `=1.5.0` |
| Last reviewed | 2026-07-27 |

Upstream repositories: [spec/schema](https://github.com/agentclientprotocol/agent-client-protocol), [Rust SDK](https://github.com/agentclientprotocol/rust-sdk).

The SDK's exact schema pin caps our effective spec level. The direct schema dependency exists only to turn on schema-level unstable flags the SDK facade does not forward, and it works because cargo unifies features on a single schema crate. Pinning it past the SDK's requirement would put two schema crates in the graph, at which point the flags no longer apply to the types the SDK actually re-exports, so schema releases above the SDK's pin are tracked as gaps here rather than adopted early.

## Companion crates

The Rust SDK repository publishes more than the core crate, so tracking only `agent-client-protocol` hides capability that upstream already ships. The freshness task reads the recorded versions straight out of this table, so it stays the one place they live.

Most companions release in lockstep with the core crate: `-http`, `-conductor`, `-polyfill`, `-derive`, and `-trace-viewer` have carried the core's exact version number on the same day since 1.0.0, so a core bump is a reliable signal that they moved too. Three do not follow that pattern and must be read on their own terms: `-schema` lives in the spec repository and versions independently (the SDK pins it exactly, see above), `-rmcp` carries its own major (3.0.0), and `-tokio` has not released since 2026-04-21.

| Crate | Latest | Adopted | Position |
| --- | --- | --- | --- |
| `agent-client-protocol` | 2.0.0 | yes | The core dependency this document tracks. |
| `agent-client-protocol-schema` | 1.6.0 | yes (1.5.0) | Capped by the SDK's exact pin, see above. |
| `agent-client-protocol-derive` | 2.0.0 | transitive | Pulled in by the core crate; no direct dependency. |
| `agent-client-protocol-http` | 2.0.0 | yes (`server` feature) | Official axum-based Streamable HTTP + WebSocket transport. It now serves all remote traffic: `main` builds `AcpHttpServer` over a per-connection `NatsAgentComponent` (`acp-nats-server/src/component.rs`), which retired the hand-rolled `transport.rs` and `connection.rs` (3,052 lines with their tests). `compat.rs` layers back the three behaviors upstream omits: `Origin` enforcement on every verb (upstream checks it server-side only on the WebSocket upgrade), `Acp-Protocol-Version` validation against the negotiated version (the transport spec says clients SHOULD send it and upstream never reads it), and `X-Accel-Buffering: no` on SSE. Drain is handled inside the component, which selects on a shutdown watch. Client feature deliberately not enabled: it wants reqwest 0.13 against the workspace's `=0.12.28`. See "Remote transport behavior changes" below. |
| `agent-client-protocol-polyfill` | 2.0.0 | no | MCP-over-ACP polyfill for agents that accept HTTP but not ACP-transport MCP servers. Relevant only once `mcp/connect`, `mcp/message`, and `mcp/disconnect` are routed (see the MCP-over-ACP row below). |
| `agent-client-protocol-rmcp` | 3.0.0 | no | Bridges `rmcp`-based MCP servers into the SDK's MCP server framework. Candidate for `crates/mcp`, not for the ACP bridge. |
| `agent-client-protocol-conductor` | 2.0.0 | no | Binary and library for stdio proxy chains using the `_proxy/successor` envelope. Different problem shape from a message-bus bridge. |
| `agent-client-protocol-tokio` | 0.11.1 | no | Not updated since 2026-04-21 and superseded by the core crate's own runtime-agnostic surface. Treated as dormant; a release would be worth investigating. |
| `agent-client-protocol-trace-viewer` | 2.0.0 | no | Developer tooling, no runtime role. |

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

## Policy

Opt in to unstable spec features ahead of stabilization. The default for every unstable feature is to enable the flag, wire the routing, and test it. Opting out is the exception and requires a rationale in the matrix below.

## Why this matters here

The [bridge](../glossary/bridge) decodes every message into typed SDK structs and re-serializes them (`acp-nats/src/wire.rs`). Fields the pinned SDK does not model are silently stripped in transit, and unknown `session/update` variants fail decode. Spec lag means silent data loss, not graceful passthrough, so this matrix must stay accurate.

## Conformance matrix

Status values: `implemented` (routed, typed, tested), `capabilities implemented` (capability payloads round-trip, but the methods behind them are not routed), `half-wired` (one half of the path is routed and tested, but the end-to-end flow cannot complete in a release build, so the surface does not work in production), `unwired` (SDK flag enabled but no routing), `dropped` (peers may send it, the bridge strips or rejects it), `unrepresentable` (pinned SDK cannot express it), `not supported` (deliberate opt-out with rationale), `watch-only` (tracked for adoption, deliberately not implemented while upstream still churns).

A green test suite does not earn a status of `implemented`. `half-wired` exists because a surface can be fully unit-tested on both sides of a seam that nothing connects in a release build.

### Agent-side methods (client to agent)

| Spec surface | Spec stage (schema 1.5.0) | Our status | Notes |
| --- | --- | --- | --- |
| `initialize` | stable | implemented | |
| `authenticate` | stable | implemented | `unstable_auth_methods` shapes enabled |
| `logout` | stable (0.13.3) | implemented | |
| `session/new` | stable | implemented | includes `additionalDirectories` |
| `session/load` | stable | implemented | includes `additionalDirectories` |
| `session/list` | stable | implemented | |
| `providers/list` | unstable (0.11.7) | unrepresentable | not a routing gap: bridge-owned `AgentHandler::list_providers` and NATS subject routing (`providers.list`) are implemented and tested, but `agent-client-protocol` 2.0.0 cannot express the request at the byte-stream boundary (no `unstable_llm_providers` feature, and the provider variants the schema does define are omitted from the SDK's `ClientRequest` method table); blocked on upstream SDK support |
| `providers/set` | unstable (0.11.7) | unrepresentable | see `providers/list`; `AgentHandler::set_provider` and NATS subject routing (`providers.set`) implemented and tested |
| `providers/disable` | unstable (0.11.7) | unrepresentable | see `providers/list`; `AgentHandler::disable_provider` and NATS subject routing (`providers.disable`) implemented and tested |
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
| JSON-RPC batches (inbound) | stable (SDK 2.0.0) | implemented | the SDK splits an inbound batch into independent dispatches and regroups the replies into one response array, so every routed method works inside a batch and unrouted ones still answer `method_not_found`; round-trip tested at the boundary. NATS carries one JSON-RPC message per subject message, so a batch never reaches the runner as a batch, and the bridge never emits one: the SDK sends typed requests and notifications individually. Every boundary now shares one implementation: HTTP, WebSocket, and stdio all hand whole frames to the SDK, so batch splitting and regrouping happen in exactly one place. This also retired a divergence the hand-rolled HTTP path carried, where a batch yielding a single JSON outcome answered with a bare response object instead of a one-element array, which JSON-RPC 2.0 does not allow |
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
| `elicitation/create` | unstable | implemented | `unstable_elicitation` SDK flag; routed both through the bridge-owned `ClientHandler::elicitation_create` and the SDK byte-stream boundary (`AgentRequest::CreateElicitationRequest`); `ElicitationScope::Request` (pre-session, no session id) is not routable since all NATS client subjects and `NatsClientProxy` construction are session-scoped |
| `elicitation/complete` (notification) | unstable | implemented | `unstable_elicitation` SDK flag; routed as a `ClientHandler::elicitation_complete` notification |
| `ext/*` | stable | implemented | passthrough |
| `ext/session/prompt_response` (bridge extension, not spec) | n/a | half-wired | The receive half is routed and unit-tested: `acp-nats/src/client/ext_session_prompt_response.rs` decodes the notification and correlates it on `meta.prompt_id`. The completion half does not exist in a release build. `PendingSessionPromptResponseWaiters::register_waiter` is the only method that inserts into the waiter map and it has been `#[cfg(test)]` since the module landed in #477, so `resolve_waiter` always returns `false` and every notification is logged and dropped at `ext_session_prompt_response.rs:96`. Nothing inserts into the `timed_out` map in any build either, so `PROMPT_TIMEOUT_WARNING_SUPPRESSION_WINDOW` never suppresses and that warning fires once per dropped response. Completing this needs a production registration site on the prompt path. The machinery is deliberately retained, not deleted: the problem it solves is real, since a notification can arrive on a session subject with no live caller and outlive the caller that was waiting for it. |

### Payload-level capabilities

| Spec surface | Spec stage | Our status | Notes |
| --- | --- | --- | --- |
| `additionalDirectories` (session/new, session/load) | stable (0.13.5) | implemented | round-trip tested through the bridge |
| Message IDs on chunks | stable (0.13.6) | implemented | 1.4.0 shape, round-trip tested |
| Session usage updates | stable (0.13.6) | implemented | 1.4.0 shape, round-trip tested |
| Session config options | stable | implemented | 1.4.0 shape, `ConfigOptionUpdate` round-trip tested |
| Boolean config options | stable (1.3.0) | implemented | stabilized shape, round-trip tested |
| `model_config` option category | stable (1.1.0) | implemented | round-trip tested |
| NES (next edit suggestions) | unstable | capabilities implemented | capability payloads round-trip via schema-level flag; NES document sync methods are not routed (no runner demand yet, revisit with Phase 4 adoption cadence) |
| Plan operations | unstable (0.13.4) | implemented | `PlanUpdate`/`PlanRemoved` round-trip tested via schema-level flag |
| Providers | unstable (0.11.7) | unrepresentable | see `providers/list`/`providers/set`/`providers/disable` rows above |
| MCP-over-ACP message types | unstable (0.13.0) | implemented | `McpServer::Acp` and `McpCapabilities.acp` payload round-trip tested via schema-level and `unstable_mcp_over_acp` SDK flag; SDK 2.0.0 dropped its own wire types in favor of the schema-native ones the bridge already used; the `mcp/connect`, `mcp/message`, `mcp/disconnect` RPC methods are not routed (no runner demand yet, revisit with Phase 4 adoption cadence) |
| Elicitation enum option descriptions | unstable (1.4.0) | implemented | `EnumOption` descriptions on `StringPropertySchema.one_of` round-trip tested |
| Tool call `name` | unstable (1.6.0) | unrepresentable | `unstable_tool_call_name` adds `name` to `ToolCall` and `ToolCallUpdateFields`, but the flag only exists in schema 1.6.0 and the pinned SDK requires schema `=1.5.0`; enabling it early would fork the schema crate and silently disarm every other schema-level flag. Adopt in the SDK release that moves to schema 1.6.0; until then the field is dropped by typed re-encode |
| Protocol v2 | unstable, heavy churn | watch-only | adopt once upstream marks it preview; the freshness workflow surfaces every release it churns in |

## Upgrade ritual

A version bump of `agent-client-protocol` (or the schema it bundles) is never just a version change. Every bump PR must:

1. Diff the schema changelog between the old and new pinned versions ([changelog](https://github.com/agentclientprotocol/agent-client-protocol/blob/main/CHANGELOG.md)).
2. For each added or stabilized method: add subject mapping in `acp-nats/src/nats/parsing.rs`, a handler (bridge trait method plus its match arm in the boundary dispatch and [NATS](../glossary/nats) dispatch), and tests, or add a matrix row with an opt-out rationale. The byte-stream boundary no longer needs per-method registrations: `connect_agent_boundary` routes through the SDK's `ClientRequest`/`ClientNotification` enums, so a method missing from its match answers `method_not_found` instead of being silently unreachable (see [ADR#0020](../adr/0020-acp-sdk-1x-boundary-and-bridge-traits.md), amendment of 2026-07-09).
3. For each added field or `session/update` variant: add a round-trip test through the bridge. Typed re-encode means unmapped fields are silently dropped, so a green compile proves nothing about coverage.
4. For each new unstable flag: enable it per the opt-in policy and wire it. A flag that only exists in a schema release above the SDK's exact pin cannot be enabled; move the direct schema dependency in lockstep with the SDK's requirement and record the gap as a matrix row instead.
5. Update the `## Companion crates` table with the new versions, and re-evaluate every row marked `no`. Companions release in lockstep with the core crate, so a bump is exactly when a gap that blocked adoption may have closed. The freshness task parses that table, so leaving it stale disables the check.
6. Update this document (matrix and spec position table) in the same PR.

The scheduled freshness workflow (`.github/workflows/acp-freshness.yml`) embeds this checklist in the issue it files when drift is detected.

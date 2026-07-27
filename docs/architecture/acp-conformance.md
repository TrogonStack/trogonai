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

## Policy

Opt in to unstable spec features ahead of stabilization. The default for every unstable feature is to enable the flag, wire the routing, and test it. Opting out is the exception and requires a rationale in the matrix below.

## Why this matters here

The [bridge](../glossary/bridge) decodes every message into typed SDK structs and re-serializes them (`acp-nats/src/wire.rs`). Fields the pinned SDK does not model are silently stripped in transit, and unknown `session/update` variants fail decode. Spec lag means silent data loss, not graceful passthrough, so this matrix must stay accurate.

## Conformance matrix

Status values: `implemented` (routed, typed, tested), `capabilities implemented` (capability payloads round-trip, but the methods behind them are not routed), `unwired` (SDK flag enabled but no routing), `dropped` (peers may send it, the bridge strips or rejects it), `unrepresentable` (pinned SDK cannot express it), `not supported` (deliberate opt-out with rationale), `watch-only` (tracked for adoption, deliberately not implemented while upstream still churns).

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
| JSON-RPC batches (inbound) | stable (SDK 2.0.0) | implemented | the SDK splits an inbound batch into independent dispatches and regroups the replies into one response array, so every routed method works inside a batch and unrouted ones still answer `method_not_found`; round-trip tested at the boundary. NATS carries one JSON-RPC message per subject message, so a batch never reaches the runner as a batch, and the bridge never emits one: the SDK sends typed requests and notifications individually |
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
| `ext/*` | stable | implemented | passthrough, plus bullard-specific `ext/session/prompt_response` |

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
5. Update this document (matrix and spec position table) in the same PR.

The scheduled freshness workflow (`.github/workflows/acp-freshness.yml`) embeds this checklist in the issue it files when drift is detected.

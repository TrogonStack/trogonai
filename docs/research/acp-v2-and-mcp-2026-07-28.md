# ACP v2 and MCP 2026-07-28

Status: research snapshot as of 2026-07-29

## Conclusion

ACP and MCP remain complementary protocol boundaries:

```text
user
  -> ACP client, usually an editor or product UI
  -> ACP agent, which owns the model and agent loop
  -> MCP client connections owned by that agent
  -> MCP servers that expose tools, resources, and prompts
```

ACP v2 makes the client-to-agent boundary more capable. MCP `2026-07-28`
makes the host-to-capability boundary stateless. Their shared JSON-RPC and
content vocabulary does not make them one domain or one lifecycle.

For this repository, the practical order is:

1. Implement the stable MCP `2026-07-28` revision through a dual-era MCP path.
2. Keep ACP wire v2 watch-only until upstream promotes it to Preview, as the
   current conformance policy already requires.
3. When ACP v2 is adoptable, add a version-selected path beside ACP v1 instead
   of replacing the existing bridge.
4. Do not merge ACP and MCP domain models. Reuse transport infrastructure and
   write explicit boundary adapters where their shared shapes are useful.

## The naming trap

Five independently versioned things are easy to conflate:

| Name | What the version identifies | Current meaning | Repository position |
| --- | --- | --- | --- |
| ACP wire v2 | Breaking JSON-RPC wire semantics negotiated as `protocolVersion: 2` | Draft, published 2026-07-20. Stable ACP remains v1. | Not enabled. The conformance matrix marks protocol v2 `watch-only`. |
| ACP Rust SDK 2.0.0 | Rust API and transport artifact version | Keeps the stable ACP v1 wire unchanged while changing Rust APIs and adding opt-in draft v2 types. | Exactly pinned at `=2.0.0`; `unstable_protocol_v2` is not enabled. |
| MCP `2026-07-28` | Date-versioned MCP wire revision | Current stable MCP revision. It is not officially named "MCP protocol v2." | Not implemented by the current MCP bridge. |
| MCP SDK v2 | Major version of an SDK package family | The official TypeScript SDK v2 is the stable SDK line implementing MCP `2026-07-28`. | Not a Rust dependency and not a wire-version declaration. |
| `rmcp` 2.x and 3.x | Major versions of the official MCP Rust SDK crate | `rmcp` 2.0.0 aligned its model types with MCP `2025-11-25`; `rmcp` 3.0.0 supports the `2026-07-28` method set. | The manifest declares the 3.x line with `version = "3.0.0"`; the current lock resolves 3.0.1. |

The ACP Rust SDK release is explicit that its 2.0 major does not mean ACP wire
v2. The SDK exposes an `unstable_protocol_v2` feature, but
`rsworkspace/Cargo.toml` enables other unstable features without enabling that
one. This agrees with
[`acp-conformance.md`](../architecture/acp-conformance.md).

The `rmcp` distinction is equally important. The official
[`rmcp` 2.0.0 release](https://github.com/modelcontextprotocol/rust-sdk/releases/tag/rmcp-v2.0.0)
says that it aligns model types with MCP `2025-11-25`. The official
[`rmcp` 3.0.0 release](https://github.com/modelcontextprotocol/rust-sdk/releases/tag/rmcp-v3.0.0)
recognizes the 2026 methods. A dependency major is evidence about an SDK
surface, not a substitute for checking the negotiated wire revision and the
repository's own routing table.

## Protocol responsibilities

| Concern | ACP v2 | MCP `2026-07-28` |
| --- | --- | --- |
| Roles | ACP Client talks to an ACP Agent. | An MCP Host owns one MCP Client per MCP Server. |
| Model ownership | The Agent normally owns model access, orchestration, modes, model selection, and reasoning configuration. | The Host owns the primary LLM integration. Sampling is deprecated, and new servers are directed toward provider APIs when they need their own model. |
| Conversation state | Durable agent sessions, canonical message IDs, replay, plans, tool-call display, and foreground state. | No protocol-level session and no whole conversation. Cross-call state uses explicit application handles. |
| Initialization | Mandatory `initialize`, with integer major-version negotiation and connection capabilities. | No initialization handshake in the modern revision. Version, client identity, and capabilities travel on every request. `server/discover` provides optional up-front discovery. |
| Prompt lifecycle | `session/prompt` acknowledges acceptance. Messages and `running`, `requires_action`, or `idle` state arrive through `session/update`; background updates can continue while idle. | A client calls a server method. A server that needs more input returns `InputRequiredResult`, and the client retries the request with input responses. |
| Message direction | Bidirectional JSON-RPC requests and notifications support permissions, elicitation, and session updates. | Clients send requests. Servers return responses and request-scoped notifications. Server-initiated work is represented through multi-round-trip results rather than free-standing requests. |
| Transports | Stdio is the stable baseline. Streamable HTTP and WebSocket remain a separate remote-transport draft. | Stdio and Streamable HTTP are stable bindings for the stateless protocol. |
| Authentication | The Agent advertises methods and the Client invokes `auth/login` and `auth/logout`. Authentication is agent-facing. | HTTP authorization treats the MCP server as an OAuth resource server and the MCP client as an OAuth client. Stdio normally obtains credentials from the environment. |
| Tools | The Agent executes a model-requested action and reports progress, content, locations, and status to the Client. | The MCP Server defines tools; the MCP Client discovers them with `tools/list` and invokes them with `tools/call`. |
| Asynchronous work | Session updates can outlive a prompt request, and the Agent can keep reporting background work while idle. | Core requests are stateless. The optional Tasks extension supplies durable handles, polling, input, progress, and cancellation for long-running server operations. |

The same process can therefore be an ACP Agent and an MCP Host. That does not
make an MCP Server an ACP Agent, and neither core protocol is a general
agent-to-agent protocol.

## How ACP composes with MCP

### Content blocks are reused, not unified

ACP was designed to reuse MCP types where useful. ACP v2 aligns its text,
image, audio, resource-link, embedded-resource, annotation, and icon shapes
with the MCP specification. ACP still wraps those blocks in ACP messages,
sessions, tool-call updates, and permission flows. MCP wraps them in MCP
resources, prompts, tool results, and request results.

This repository should continue to treat `agent_client_protocol::schema::*`
and `rmcp::model::*` as protocol-boundary types. Structurally similar content
can be converted explicitly at a composition boundary. It should not become a
shared domain type merely because both upstream schemas share ancestry.

### ACP can supply MCP servers to an agent session

ACP session lifecycle requests can carry `mcpServers`. The ACP Client supplies
configuration, while the ACP Agent connects as the MCP-side host/client. This
is the normal composition:

```text
ACP Client
  -> session/new or session/resume with MCP server configuration
ACP Agent
  -> MCP discovery and calls
MCP Server
  -> tools, resources, and prompts
ACP Agent
  -> ACP tool-call and message updates for the Client UI
```

ACP v2 makes this boundary more explicit by removing the v1 Client filesystem
and terminal execution RPCs. A Client that wants to expose editor state, file
access, or command execution should provide an MCP server. The Agent still
owns execution policy and reports its activity through ACP.

### ACP `ToolCall` is not MCP `tools/call`

An ACP `tool_call_update` is agent-to-client state for progress, rendering,
permission context, raw input and output, diffs, terminal display, and affected
locations. It does not define how the underlying tool is discovered or invoked.

MCP `tools/call` is the actual capability invocation against an MCP Server. A
single MCP call may produce a corresponding ACP tool-call update, but that
correlation belongs to the Agent implementation. ACP also reports built-in
tools and subagent launches that have no MCP call at all.

### MCP-over-ACP is unstable

The MCP-over-ACP RFD proposes an `acp` MCP transport that tunnels MCP messages
through an existing ACP channel. It is a transport bridge, not a role or trust
merge. The RFD remains a proposal, its schema surface is explicitly unstable,
and some examples still use pre-v2 ACP capability shapes.

This repository currently enables `unstable_mcp_over_acp`. The conformance
matrix records that `McpServer::Acp` and the capability payload round-trip, but
`mcp/connect`, `mcp/message`, and `mcp/disconnect` are not routed. That is an
accurate capability gap, not a reason to make MCP-over-ACP part of the ACP v2
adoption path.

## Accepted repository decisions

This section records existing accepted decisions. It is not a new
recommendation.

### ADR 0003: keep protocol, SDK, transport, and backbone names separate

[`ADR 0003`](../adr/0003-ai-protocol-transport-taxonomy.md) defines ACP and
MCP as protocols, SDKs as role toolkits, stdio and HTTP as transports, and NATS
as the internal backbone. The five versions in the naming table must therefore
remain separate in code, documentation, dependency updates, and telemetry.

### ADR 0004: keep protocol types at protocol boundaries

[`ADR 0004`](../adr/0004-protocol-and-transport-layering.md) requires protocol
dispatchers to translate into application and domain concepts. Shared content
shapes do not authorize ACP types in MCP domain code or MCP types in ACP domain
code. A combined runtime may compose both role SDKs while preserving their
boundaries.

### ADR 0011: share the JSON-RPC transport seam, not protocol semantics

[`ADR 0011`](../adr/0011-jsonrpc-over-nats-binding.md) gives ACP and MCP one
lossless JSON-RPC-over-NATS codec. The subject carries the method, headers carry
JSON-RPC control fields, and the body carries params, result, or error detail.
That codec remains useful for both new revisions. Each protocol still injects
its own typed decode and method-to-subject mapping.

### ADR 0020: retain bridge-owned callback traits and the NATS model

[`ADR 0020`](../adr/0020-acp-sdk-1x-boundary-and-bridge-traits.md) keeps the
subject-routed, durable, multi-peer NATS transport instead of forcing it into an
SDK byte-stream abstraction. The ACP SDK 2.0 amendment confirms that the
bridge-owned `AgentHandler` and `ClientHandler` seam survived the SDK major
bump. ACP wire v2 should enter through an additional version-selected adapter,
not by undoing that decision.

### ADR 0021: typed decode makes every protocol bump an explicit migration

[`ADR 0021`](../adr/0021-typed-decode-over-passthrough-forwarding.md) rejects
raw passthrough. Unknown fields can be stripped and unknown variants can fail
decode until the pinned SDK models them. ACP v2 support therefore requires
typed routing and round-trip coverage for every changed method, field, content
variant, and update shape. Negotiating `protocolVersion: 2` without those types
would be incorrect.

### ACP conformance policy: v2 is watch-only until Preview

[`acp-conformance.md`](../architecture/acp-conformance.md) is the source of
truth. It currently records:

- wire protocol v1
- `agent-client-protocol` 2.0.0
- effective schema 1.5.0
- unstable features enabled by default except where explicitly justified
- protocol v2 as `watch-only`, with adoption deferred until Preview

The current recommendation below follows that accepted policy.

## Exact impact on this checkout

| Artifact | Current evidence | Impact |
| --- | --- | --- |
| `rsworkspace/Cargo.toml` | ACP SDK is exactly `=2.0.0`; its enabled features omit `unstable_protocol_v2`. The direct ACP schema is exactly `=1.5.0`. `rmcp` declares `version = "3.0.0"` without an exact pin. | ACP wire v2 is not present. The MCP SDK dependency is already on the 3.x line, so the remaining gap is wire-level, not dependency-level. |
| `rsworkspace/Cargo.lock` | Resolves ACP SDK 2.0.0, ACP schema 1.5.0, and `rmcp` 3.0.1. | Documentation must distinguish the manifest's 3.x declaration from the resolved patch/minor release. |
| [`acp-conformance.md`](../architecture/acp-conformance.md) | ACP v1 methods and payloads are individually tracked; v2 is watch-only. MCP-over-ACP payloads are present but its RPC methods are unwired. | Any ACP upgrade must update the matrix in the same change and follow its round-trip ritual. |
| `rsworkspace/crates/acp/acp-nats/src/agent/prompt.rs` | Creates notification and response consumers before publishing a `PromptRequest`, then loops until a v1 `PromptResponse` carries the turn's `StopReason`. | This is a v1 turn-owned lifecycle. ACP v2 needs immediate prompt acknowledgement and a session-scoped state/update path independent of the pending prompt request. |
| `rsworkspace/crates/acp/acp-nats/src/jetstream/consumers.rs` | Prompt notifications and final responses are filtered by session plus request ID on separate streams. | Preserve these consumers for v1. A v2 path cannot use final prompt response arrival as the end-of-work signal; it must consume `state_update` and background updates even while idle. |
| `rsworkspace/crates/mcp/mcp-nats/src/transport.rs` | `method_suffix` and `method_from_suffix` are explicit allowlists for the legacy lifecycle and 2025 task surface. | An `rmcp` bump alone cannot add MCP `2026-07-28`. Routing must add modern methods and retain a legacy table for older peers. |
| `rsworkspace/crates/mcp/mcp-nats/src/nats/parsing.rs` | Typed subject parsing mirrors the same request and notification vocabulary. | Subject parsing and tests must change with the transport allowlist. |

### ACP prompt lifecycle delta

The current v1 path has this ownership:

```text
session/prompt request
  -> durable command
  -> per-request session/update consumer
  -> per-request PromptResponse consumer
  -> StopReason ends the pending request
```

ACP v2 changes it to:

```text
session/prompt request
  -> immediate empty acknowledgement
session/update
  -> canonical user message
  -> running, requires_action, or idle state
  -> idle state carries the stop reason
background session/update
  -> may continue while idle and outside any prompt request
```

The correct compatibility design is two protocol paths selected after ACP
initialization. Reusing the same durable NATS streams is possible, but v2
updates need session and message identity independent of the request-scoped
consumer that defines v1 turns.

### MCP method-table delta

The current MCP table includes legacy methods such as:

- `initialize` and `notifications/initialized`
- `ping` and `logging/setLevel`
- `resources/subscribe` and `resources/unsubscribe`
- `tasks/list` and `tasks/result`
- direct server requests such as `sampling/createMessage`, `roots/list`, and
  `elicitation/create`

MCP `2026-07-28` requires a modern path that includes:

- `server/discover`
- per-request protocol version, identity, and capabilities
- `subscriptions/listen`
- multi-round-trip input results rather than free-standing server requests
- the current Tasks extension surface, including `tasks/update` and no
  `tasks/list` or blocking `tasks/result`

Legacy peers still require the old table. The NATS subject mapper should select
an era-specific vocabulary rather than deleting legacy methods globally.

## Recommendations

These are research recommendations. They are not accepted ADR decisions unless
an existing policy is explicitly cited.

### 1. Implement stable MCP work first

The Rust MCP integration is already on the `rmcp` 3.x line, so the remaining
work is to implement MCP `2026-07-28` as a modern path while retaining the
current legacy path. Treat this as a protocol migration, not a dependency-only
update.

The work should include:

1. Modern and legacy lifecycle selection at each stdio or HTTP boundary.
2. `server/discover`, per-request metadata, current HTTP headers, and
   `subscriptions/listen` at the edges that need them.
3. Era-aware NATS method mapping and subject parsing.
4. Current Tasks extension mapping rather than the removed experimental core
   methods.
5. Conformance tests for modern and legacy clients and servers.

The shared codec from ADR 0011 should remain unchanged unless a failing
round-trip proves otherwise.

### 2. Keep ACP v2 watch-only until Preview

Do not enable `unstable_protocol_v2` while ACP v2 is Draft. Continue tracking
upstream releases through the freshness workflow. Begin implementation when
upstream marks v2 Preview, which is the threshold already recorded in the
conformance matrix.

At that point:

1. Keep ACP v1 available.
2. Select v1 or v2 after `initialize` negotiation.
3. Add v2-specific callback and routing adapters rather than changing the v1
   trait contract in place.
4. Keep v1 `fs/*`, `terminal/*`, `session/load`, and turn-ending prompt
   behavior for v1 peers.
5. Add v2 `auth/*`, resume replay, config-option modes, agent-owned terminal
   display, state updates, and upsert semantics only on the v2 path.
6. Follow ADR 0021 with field and variant round-trip tests before advertising
   the capability.

### 3. Keep MCP-over-ACP out of the critical path

Continue representing its payloads for compatibility, but do not route or
advertise the transport as complete until the RFD and its v2 capability shapes
stabilize or a concrete runner requires it.

### 4. Do not merge ACP and MCP domains

Maintain separate ACP session, prompt, tool-call display, permission, and
message types from MCP discovery, tool invocation, resource, prompt-template,
and task types. Share only infrastructure whose contract is genuinely common:

- JSON-RPC codec and error envelope mechanics
- NATS publishing, correlation, telemetry, and backpressure primitives
- explicit content conversion helpers at a composition boundary

Do not reuse ACP session IDs as MCP state handles, treat ACP `ToolCall` as an
MCP call envelope, or place MCP server lifecycle inside the ACP domain model.

## Official external sources

### ACP

- [ACP v2 Draft announcement](https://agentclientprotocol.com/announcements/acp-v2-draft)
- [ACP v2 migration guide](https://agentclientprotocol.com/protocol/v2/migration)
- [ACP architecture](https://agentclientprotocol.com/get-started/architecture)
- [ACP v2 tool calls](https://agentclientprotocol.com/protocol/v2/tool-calls)
- [MCP-over-ACP proposal](https://agentclientprotocol.com/rfds/mcp-over-acp)
- [ACP specification and schema repository](https://github.com/agentclientprotocol/agent-client-protocol)
- [ACP Rust SDK repository](https://github.com/agentclientprotocol/rust-sdk)
- [ACP Rust SDK 2.0.0 release](https://github.com/agentclientprotocol/rust-sdk/releases/tag/v2.0.0)

### MCP

- [MCP `2026-07-28` specification](https://modelcontextprotocol.io/specification/2026-07-28)
- [MCP `2026-07-28` key changes](https://modelcontextprotocol.io/specification/2026-07-28/changelog)
- [MCP architecture](https://modelcontextprotocol.io/specification/2026-07-28/architecture)
- [MCP versioning and compatibility](https://modelcontextprotocol.io/specification/2026-07-28/basic/versioning)
- [MCP tools](https://modelcontextprotocol.io/specification/2026-07-28/server/tools)
- [MCP Tasks extension](https://modelcontextprotocol.io/extensions/tasks/overview)
- [MCP specification and documentation repository](https://github.com/modelcontextprotocol/modelcontextprotocol)
- [MCP TypeScript SDK repository](https://github.com/modelcontextprotocol/typescript-sdk)
- [MCP Rust SDK repository](https://github.com/modelcontextprotocol/rust-sdk)
- [`rmcp` 2.0.0 release](https://github.com/modelcontextprotocol/rust-sdk/releases/tag/rmcp-v2.0.0)
- [`rmcp` 3.0.0 release](https://github.com/modelcontextprotocol/rust-sdk/releases/tag/rmcp-v3.0.0)

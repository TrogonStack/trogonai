# Synthesis: the ACP protocol contract and ecosystem

Fifteen product dossiers, one protocol, one question: what does the Agent
Client Protocol actually require, and how does the ecosystem actually use
it? This synthesis is frozen as decision-time input: where a conclusion here
differs from an accepted record in the [ADR index](../../adr/index.md) or
from the current spec position in
[ACP Conformance](../../architecture/acp-conformance.md), the ADR or the
conformance document is authoritative.

## Headline findings

1. **Every major coding agent CLI ships an ACP agent mode today.** Gemini CLI
   (`gemini --acp`), Codex (official `codex-acp` adapter), Claude Code
   (official `claude-agent-acp` adapter), Goose (`goose acp`), OpenCode
   (`opencode acp`), Cline (`cline --acp`), Cursor (`cursor-agent agent
   acp`), Grok Build (`grok agent stdio`), Devin (`devin acp`), Hermes
   (`hermes acp`), Buzz (`buzz-agent`). All speak protocol version 1
   JSON-RPC over stdio. The only Tier 1 product studied with no ACP surface
   at all is NetClaw.
2. **The host component is mostly wiring, not protocol work.** The current
   Rust SDK already contains a subprocess helper for spawning an agent CLI as
   a stdio child. Reference designs exist for the client-host role: Block's
   `buzz-acp` harness (spawns and drives any ACP agent binary), the
   quasi-official `acpr` registry runner, and Devin Desktop (hosts Devin,
   Codex, Claude Agent, and OpenCode side by side as ACP agents).
3. **Channel mapping over ACP has ecosystem precedent.** The official
   clients directory lists messaging integrations (Discord, Slack, Telegram,
   WeChat, QQ, Lark, Matrix) as ACP clients, plus independent bridges
   (OpenACP, Sniptail, telegram-acp-bot). OpenClaw runs the same pattern
   productized: channels in front, ACP-hosted agents behind a gateway.
4. **Protocol governance moved to a community org.** The spec and SDK now
   live under `github.com/agentclientprotocol` (Zed remains maintainer of
   record, Apache-2.0, no CLA). A public ACP Registry launched co-branded
   with JetBrains. Client-side adoption spans Zed, JetBrains AI Assistant,
   VS Code extensions, four Neovim plugins, Emacs, Obsidian, Marimo, and
   more; the registry lists 50+ agents.

## Protocol contract (wire v1, stable)

- JSON-RPC 2.0 with Methods (request/response) and Notifications (one-way),
  primarily over stdio to a client-spawned agent subprocess; HTTP/WebSocket
  transports exist for remote agents.
- Session lifecycle: `session/new`, `session/load` (restore with replay),
  then `session/prompt` turns that stream `session/update` notifications
  (message chunks, tool calls, plans) and end with a StopReason
  (`end_turn`, `max_tokens`, `max_turn_requests`, `refusal`, `cancelled`).
  `session/cancel` is a notification acknowledged via
  `StopReason::Cancelled` from the still-pending prompt response.
- Bidirectional capability negotiation at `initialize`: clients declare
  `clientCapabilities` (fs read/write, terminal, content types); agents
  declare `agentCapabilities` (`loadSession`, `mcpCapabilities`,
  `promptCapabilities` for image/audio/embedded-context multimodal content
  blocks). Optional features exist only if the counterpart advertised them.
- Mid-turn the agent calls back into the client: `fs/read_text_file`,
  `fs/write_text_file` (text-only; no binary RPC exists), `terminal/*`, and
  `session/request_permission`, the authz hook point for tool execution.
  Tool-call lifecycle: a `tool_call` is announced pending, an optional
  `session/request_permission` runs before execution, then `tool_call_update`
  moves through `in_progress` to `completed`/`failed`.
- MCP composes underneath: `session/new` carries `mcpServers` config, and
  the agent consumes tools via MCP. MCP-over-ACP (tunneling MCP through the
  ACP channel via `mcp/connect`, `mcp/message`, `mcp/disconnect`) is an
  unaccepted Draft RFD split out of the proxy-chains proposal.
- Extensibility: every type carries `_meta` (custom data must nest under a
  namespaced key; root keys are reserved for W3C trace context); custom
  methods are underscore-prefixed (for example `_zed.dev/workspace/buffers`).
  No spec convention exists for user/channel identity metadata.

## v1 vs v2 draft (breaking-change diff)

v2 is an explicit, intentionally breaking Draft (opt-in, unstable, no
timeline; maintainers warn against production use). Confirmed breaking
changes:

- Removes the deprecated HTTP+SSE MCP transport; MCP server configs require
  an explicit `type` discriminator; stdio MCP becomes an opt-in
  `session.mcp.stdio` capability.
- Removes `session/load` in favor of `session/resume` with an optional
  `replayFrom` cursor.
- Removes the client-owned filesystem/terminal RPC surface
  (`clientCapabilities.fs`, `fs/*`, `terminal/*`) in favor of agent-owned,
  display-only terminal output referenced by a `terminalId`.

Systems leaning on client fs/terminal or `session/load` face a rewrite, not a
patch, when v2 eventually stabilizes: those are exactly the surfaces the
recommended `acp-host` component (see the [decision record](./decision-record.md))
needs to serve today. Building them as swappable modules keeps a v2 path
adoptable later without a structural rewrite. The watch-only stance recorded
in [ACP Conformance](../../architecture/acp-conformance.md) remains correct.

## Governance and ecosystem adoption

Spec and SDKs live under the community `agentclientprotocol` GitHub org
(Apache-2.0, RFD process, joint lead maintainers from Zed and JetBrains; no
foundation donation). Adoption: 50+ agents speak it (Gemini CLI, Codex and
Claude Code via official adapters, Goose, Cline, Cursor, Devin, Copilot);
clients span Zed, JetBrains AI Assistant, VS Code extensions, Neovim, Emacs,
Obsidian, Marimo, plus messaging bridges (Discord, Slack, Telegram, Matrix).
A public ACP Registry launched co-branded with JetBrains. See the
[Rust Crate Inventory](./rust-crates.md) for how the SDK family releases on
a coordinated train, and [Tier 2 Client Profiles](./tier2-profiles.md) for
the community client implementations (Neovim, Emacs, Obsidian, Marimo,
OpenHands).

## The host-role pattern

The caller into an ACP agent is always a client host: a process that spawns
the agent CLI as a stdio child (with credentials injected as environment
variables), speaks the client role over its stdio, serves the client-owned
callbacks (fs, terminal, permission prompts), and renders the update stream.
Zed's `agent_servers` config, JetBrains' `acp.json`, Block's `buzz-acp`
harness, and Devin Desktop all occupy this seat. Adapters add one hop for
vendors without native support: host to `claude-agent-acp` to the Claude
Agent SDK; host to `codex-acp` to the Codex engine. Full detail, including
Buzz's directionality and the reference designs worth studying, is in
[Host Role and Invocation Mechanics](./host-role-and-invocation.md).

## Channel integration patterns

Community bridges and Buzz run Telegram/Slack/Discord adapters directly as
ACP clients, one session per conversation. But both production
multi-channel systems studied keep the channel leg native instead: OpenClaw's
channel adapters use native gateway routing with ACP only as a swappable
execution runtime underneath, and Hermes scopes ACP to editors, running
channels through a separate typed-event gateway. The lesson is not "avoid
ACP" but "ACP alone is not a channel UX." Whichever wire is chosen, the
adapter playbook is the same across every system studied: instant
acknowledgement, named streaming modes with debouncing, a queue-mode enum
for mid-run messages, structured session keys, mention-gating in group
chats, and an approval ladder that falls back to a policy engine when
headless. See [Channel Mapping](./channel-mapping.md) for how channels map
onto ACP as clients and what changes versus editor clients, and
[Channel Bridge Mechanics](./bridge-mechanics.md) for how OpenClaw, Hermes,
Buzz, and community bridges actually implement acks, streaming, queueing,
and metadata, plus the two architectures those systems converge on.

## File and media handling

ACP can carry base64 media in content blocks, but production systems move
bytes out-of-band: download inbound media to a staged workspace or cache,
preprocess it (speech-to-text cascades, a vision branch that either passes a
raw block to multimodal models or generates an `[Image]` description, PDF
native handling vs. extraction), then hand the agent a local path. Outbound,
the agent references a path and the adapter ships it platform-natively. Buzz
keeps media in a separate object-store service entirely (Blossom on
S3/MinIO). ACP has no binary fs RPC, no size limits, and no artifact
concept; those are all adapter inventions. Full detail in
[File and Media Pipeline](./file-media-pipeline.md).

## ACP vs A2A: division of labor

ACP (client-to-agent, a host driving a local or adapter-fronted agent) and
A2A (agent-to-agent, remote peer delegation with durable tasks and
AgentCard discovery) are orthogonal by design and are not converging. The
"ACP is becoming A2A" claim traces to a different, unrelated protocol: IBM
and BeeAI's Agent Communication Protocol (also abbreviated ACP) really did
merge into A2A under LF AI & Data in 2025. Zed's Agent Client Protocol is
independently governed and its v2 draft never mentions A2A. In practice,
orchestration frameworks speak A2A and not ACP, while editor-adjacent
products speak ACP; the one crossover to watch is ACP's proxy-chains RFD
(conductor-routed intermediaries), which creeps toward multi-component
orchestration inside the client-agent seat but remains proposal-stage. Full
analysis, including the seat-by-seat comparison table, in
[ACP vs A2A](./acp-vs-a2a.md).

## Post-synthesis evidence: DeepSeek Harness (2026-08-20)

This section was added after the original fifteen-product synthesis and does
not retroactively change its frozen decision-time claims. The
[DeepSeek Harness dossier](./products/deepseek-harness.md) uses release
`dsh-v0.1.0-rc.8` at pinned commit
`141eb6fef83422698aef7a981029e843e8161534`.

DeepSeek Harness adds a particularly clear two-sided ACP example. Its native
automation server speaks wire v1 over JSON-RPC stdio, creates fresh sessions,
streams only committed assistant messages, exposes one-shot permission
decisions, and intentionally advertises no filesystem, terminal, MCP, session
load, or interactive UI capability. Its separate `dsh-subagent-acp` provider
plays the client-host role: one fresh child per run, explicit
`initialize`/`session/new`/`session/prompt`, machine permission policy, and
owned cancellation and process teardown.

The product is not callable from TrogonAI at the checked commit. Protocol
version is not the blocker: both sides use wire v1 and the required core method
subset overlaps. The missing boundary is still the planned client-side
`acp-host` that spawns and supervises the child and connects its stdio to the
NATS session surface. The exact upstream source-checkout invocation is
`DEEPSEEK_API_KEY=... pnpm --dir /path/to/deepseek-harness run demo:acp`;
upstream documents a composed repository demo rather than a standalone
`dsh acp` executable. This evidence strengthens the original host-role
decision while adding an important requirement: the host must support agents
that self-serve tools and advertise no client filesystem or terminal callbacks.

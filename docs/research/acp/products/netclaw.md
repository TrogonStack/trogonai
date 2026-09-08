# Netclaw (netclaw-dev/netclaw, Petabridge)

Produced by the ACP product case-study pass (2026-07-30), then adversarially verified; corrections below override the body where they conflict.

## Netclaw (Petabridge) -- gateway channel case study for trogonai

Netclaw is Petabridge's open-source, self-hosted, always-on agent daemon built on Akka.NET (C#, .NET 10). GitHub: [netclaw-dev/netclaw](https://github.com/netclaw-dev/netclaw). It is Apache 2.0, pre-1.0 (0.24.x line as of this dossier's July 2026 capture), created 2026-02-21. It is explicitly a different project from `automateyournetwork/netclaw` (an OpenClaw-based network tool that happens to share the name).

### 1. ACP status: none

Netclaw does not implement the Agent Client Protocol in any form. Its README, product page, and [architecture overview](https://netclaw.dev/architecture/overview/) contain zero references to ACP, `agent-client-protocol`, Zed, or `agentclientprotocol.com`, and Netclaw is absent from the [official ACP registry](https://agentclientprotocol.com/get-started/registry) of implementing agents and clients. This is not an oversight to fix later so much as a category mismatch: Netclaw's whole design center is being the durable, supervising parent process (Akka actor tree, event-sourced sessions), the role ACP expects the *client* (editor/host) to play, not the ephemeral subprocess agent ACP expects to spawn and tear down. Note the adjacent but distinct project [OpenClaw](./openclaw.md) does ship an "ACP agents" feature (`docs/tools/acp-agents.md`, `channels.discord.threadBindings.spawnAcpSessions`) that spawns external ACP coding harnesses inside chat threads -- this is sometimes conflated with Netclaw because the two share GitHub topics (`hermes-agent`, `openclaw`), but it is OpenClaw's feature, not Netclaw's.

### 2. Integration wiring

Netclaw ships two binaries: `netclawd`, an always-on ASP.NET Core daemon binding loopback `127.0.0.1:5199` by default, and `netclaw`, a thin CLI/TUI. The CLI talks to the daemon over SignalR (`/hub/session`, real-time sessions) and REST (`/api/*`, management), never as a subprocess the daemon spawns per session -- it is the reverse of ACP's stdio-subprocess model. Sessions are persistent Akka actors (`LlmSessionActor`) keyed by channel plus thread identifier (Slack thread `channelId/threadTs`), recovered via event-sourced journal replay and passivated after idle timeout. Permission requests run through a four-layer invocation stack (operation hard-deny, resource hard-deny, audience tool grant, interactive approval gate); non-interactive channels auto-deny gated tools rather than prompting, which has no ACP `session/request_permission` analog. MCP servers are configured in the layered `~/.netclaw/config/` JSON tree and managed by `McpClientManager` with OAuth support and progressive disclosure (sessions see only server summaries until a tool is invoked) -- this is MCP passthrough on the tool side, unrelated to ACP.

### 3. Channel mapping

Yes, Netclaw maps external channels into sessions, but never over ACP. Slack (Socket Mode, per-channel audience controls), Discord (guilds and DMs), and Mattermost (WebSocket events plus REST replies) are all documented production channels. The doctrine is "everything is just input" -- a Slack message, a webhook POST, a timer firing, or a CLI chat line all become messages routed to the same session-actor abstraction, with audience (Public/Team/Personal) derived from which channel the message arrived on. This is architecturally the same pattern trogonai would want for its own channel adapters, just implemented as Akka actor message routing rather than ACP session multiplexing.

### 4. Callability from trogonai today

Not callable as an ACP agent: there is no invocation command, no `--acp` flag, no adapter package, because no ACP surface exists to invoke. trogonai could only reach Netclaw today via non-ACP paths -- pointing Netclaw's own `McpClientManager` at an MCP server trogonai exposes (inverse direction), or having both systems post into a shared Slack/Discord channel with no session-level protocol between them. Building a real bridge (a SignalR client translating ACP session/prompt/permission calls onto Netclaw's session-actor and approval-gate model) is a medium-to-high effort exercise with no existing community adapter, since Netclaw's lifecycle model (long-lived daemon-owned actors) does not map cleanly onto ACP's per-session subprocess model.

### 5. Design lessons for trogonai

Copy: the audience/trust-tier classification derived purely from ingress channel (Public/Team/Personal), the four-layer tool-invocation stack where UI approval is only the outermost gate, and "everything is just input" as a single message-routing doctrine across channels. Avoid: coupling the channel-adapter layer so tightly to the daemon's own session-actor supervision that there is no clean seam for an external protocol like ACP to attach at -- trogonai's ACP boundary (`acp-nats*`) should stay a distinct, spawnable interface rather than folding channel routing and agent-hosting into one inseparable process the way Netclaw does.

### Sources

- https://github.com/netclaw-dev/netclaw
- https://netclaw.dev/
- https://netclaw.dev/architecture/overview/
- https://netclaw.dev/security/security-model/
- https://netclaw.dev/skills/overview/
- https://agentclientprotocol.com/get-started/registry
- https://docs.openclaw.ai/tools/acp-agents
- https://github.com/openclaw/openclaw/issues/31518

## Adversarial verification

- **confirmed**: 1. ACP status is "none" with mechanism: "none stated". (GitHub code search on netclaw-dev/netclaw for "ACP" and "agent-client-protocol" returned zero hits, and WebFetch of the README and https://netclaw.dev/architecture/overview/ found no mention of ACP, agent-client-protocol, Zed, or agentclientprotocol.com.)
- **confirmed**: 2. Callability verdict from a Rust ACP client host is "no": Netclaw exposes no ACP surface at all, and the only reachable paths are non-ACP (MCP-client-target inversion or shared-channel peering), with no path to spawn/supervise it as an ACP agent process. (Primary sources show Netclaw is a closed-loopback ASP.NET Core daemon (netclawd) reachable only via SignalR /hub/session and REST /api/*, is C#/.NET (not a crates.io/npm ACP SDK consumer), is absent from the official ACP registry, and exposes no --acp flag or bridge, matching the described inversion/channel-peer-only reachability.)
- **confirmed**: Netclaw README, netclaw.dev product page, and architecture-overview doc contain no mention of ACP, agent-client-protocol, Zed, or agentclientprotocol.com. (Directly fetched https://github.com/netclaw-dev/netclaw and https://netclaw.dev/architecture/overview/ and confirmed via WebFetch summarization plus GitHub code-search API (0 results for both "ACP" and "agent-client-protocol" repo-scoped) that none of these terms appear.)
- **confirmed**: Netclaw does not appear in the official ACP registry of implementing agents/clients. (Fetched https://agentclientprotocol.com/get-started/registry; the listed 40+ agents (Claude Agent, Gemini CLI, Cursor, goose, Codex, etc.) do not include Netclaw, netclaw-dev, or Petabridge.)
- **confirmed**: Netclaw is two binaries (netclawd daemon on loopback http://127.0.0.1:5199, and netclaw CLI/TUI) connecting via SignalR /hub/session and REST /api/*. (https://netclaw.dev/architecture/overview/ explicitly states netclawd is "an ASP.NET Core application that owns all agent logic" binding to "http://127.0.0.1:5199 (loopback only) by default," with the CLI connecting via SignalR to /hub/session for sessions and REST /api/* for management.)
- **refuted**: Channels documented as production integrations are Slack (Socket Mode, per-channel audience controls) and Discord (guilds + DMs); Mattermost appears only in the Aspire sample demo, not as a documented production channel. (netclaw.dev has a dedicated page at /channels/mattermost/ describing Mattermost as a fully documented production integration (WebSocket + REST, bot account, "same default-deny ACL model as Slack and Discord," systemd deployment guidance, and an explicit "Always set AllowedUserIds on a production server" instruction), so Mattermost is not confined to the Aspire demo sample; it does receive lighter architectural (actor-level) detail than Slack/Discord on the architecture-overview page specifically, but the broader claim that it is undocumented as a production channel is contradicted by primary source content.)
- **confirmed**: Sessions are Akka.NET persistent actors (LlmSessionActor) keyed by channel+thread, routed via SessionManager with child-per-entity routing; 'everything is just input'. (https://netclaw.dev/architecture/overview/ confirms sessions map to persistent actors keyed by {channelId}/{threadTs}-style identifiers, with LlmSessionActor as one-per-conversation and SessionManager doing child-per-entity message routing; Petabridge's own blog post (https://petabridge.com/blog/introducing-netclaw/) additionally corroborates Akka.NET as Petabridge's actor framework underlying Netclaw.)

### Corrections (authoritative where they conflict with the body)

Correct the Mattermost sub-claim: netclaw.dev publishes a dedicated production-integration page at /channels/mattermost/ (WebSocket events plus REST replies, bot account with the same default-deny ACL model as Slack and Discord, "Always set AllowedUserIds on a production server," systemd deployment guidance). Mattermost is not confined to the Aspire sample demo; it is a documented production channel. The narrower and defensible version of the claim is: the architecture-overview page gives Slack and Discord dedicated actor-level implementation detail (SlackGatewayActor, DiscordGatewayActor, etc.) but gives Mattermost no equivalent actor-level treatment there, only a navigation link out to its own channel page. All other claims, including the core ACP-status and callability verdicts, are confirmed against primary sources with no refuting evidence found.

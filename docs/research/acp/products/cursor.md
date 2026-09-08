# Cursor (cursor-agent CLI / Cursor IDE)

Produced by the ACP product case-study pass (2026-07-30), then adversarially verified; corrections below override the body where they conflict.

## Cursor and the Agent Client Protocol

Cursor is the dominant standalone AI code editor, built on VS Code, with its own CLI agent (`cursor-agent`, invoked as `agent`), IDE-embedded agent, and cloud "Background Agents." As of mid-2026 it has native, first-party ACP support, added recently and after initially declining to prioritize it.

### ACP status: native, both directions in spirit but shipped only as agent

As late as October 2025, Cursor's own staff answered a community forum request for ACP support with: "currently we support as the Agent protocol MCPs. We may add in the future support for ACP if there is enough interest" (https://forum.cursor.com/t/does-cursor-cli-support-acp/132783). That is, ACP did not exist in Cursor at that point; MCP was Cursor's answer to "protocol."

By March 2026, Cursor had reversed course: it joined the ACP Registry and shipped `agent acp`, a native ACP-server mode built into the same `cursor-agent` CLI binary that is already used for interactive and headless work (https://cursor.com/docs/cli/acp). This let Cursor plug into JetBrains IDEs (2025.3.2+, AI Assistant plugin, no JetBrains AI subscription required) and into Zed, both of which list Cursor as an ACP agent in their respective agent registries/panels (https://blog.jetbrains.com/ai/2026/03/cursor-joined-the-acp-registry-and-is-now-live-in-your-jetbrains-ide/, https://zed.dev/acp/agent/cursor). Cursor plays the ACP **agent** role only, exposing its own coding agent to external ACP clients ([JetBrains](./jetbrains.md), [Zed](./zed.md), custom integrations, avante.nvim on Neovim). It does not act as an ACP **client** that consumes other agents. Before this, third-party bridges existed (roshan-c/cursor-acp, raphaelluethy/cursor-acp, blowmage/cursor-agent-acp-npm), wrapping Cursor's proprietary CLI JSON output into ACP; these are now largely superseded by the native command, though not formally deprecated by Cursor.

### Integration wiring

`agent acp` is a stdio subprocess: the client (editor or bridge) spawns it, writes newline-delimited JSON-RPC 2.0 requests to stdin, and reads responses/notifications from stdout, with logs on stderr (https://cursor.com/docs/cli/acp). The documented session lifecycle is `initialize` to `authenticate` (methodId `cursor_login`) to `session/new`/`session/load` to `session/prompt`, with `session/update` streaming notifications and `session/request_permission` round trips for tool approval (client answers allow-once, allow-always, or reject-once). MCP servers are read from project- or user-level `.cursor/mcp.json`; team-level MCP servers configured via Cursor's web dashboard are explicitly not passed through in ACP mode.

### Channel mapping outside the editor

Cursor's Slack integration ("@cursor" mentions trigger a cloud VM Background Agent that reads the thread and opens a GitHub PR) is a separate, non-ACP product surface -- there is no evidence it routes through ACP sessions; it appears to hit Cursor's own Background Agent API/VM orchestration directly (https://cursor.com/docs/integrations/slack). No Telegram, WhatsApp, or voice channel mapping is documented.

### Callability from trogonai today

trogonai can spawn `agent acp` as a stdio ACP agent right now through its existing acp-nats-stdio bridge, the same shape used for any ACP-native CLI. Auth is via `CURSOR_API_KEY` (env var or `--api-key` flag) generated from Cursor Account Settings, or interactive `cursor_login`. Cursor requires a paid account/API key for meaningful usage (Pro is $20/month with a $20 credit pool); there is no free self-hosted path (https://cursor.com/pricing, https://cursor.com/terms/pricing).

### Design lessons

Copy: shipping ACP as a mode flag on an existing CLI binary rather than a separate package keeps versioning simple, and the explicit `session/request_permission` tri-state (allow-once/allow-always/reject-once) is a clean primitive worth mirroring. Avoid: gating MCP passthrough so team-level config is invisible in ACP mode is a real seam trogonai should not repeat, and treating chat-channel integrations (Slack) as an entirely separate stack from ACP forfeits a chance at a single session abstraction across surfaces.

### Sources

- https://cursor.com/docs/cli/acp
- https://cursor.com/docs/cli/headless
- https://cursor.com/docs/cli/reference/parameters
- https://cursor.com/docs/integrations/slack
- https://forum.cursor.com/t/does-cursor-cli-support-acp/132783
- https://blog.jetbrains.com/ai/2026/03/cursor-joined-the-acp-registry-and-is-now-live-in-your-jetbrains-ide/
- https://zed.dev/acp/agent/cursor
- https://cursor.com/pricing
- https://cursor.com/terms/pricing
- https://github.com/roshan-c/cursor-acp
- https://github.com/raphaelluethy/cursor-acp
- https://github.com/blowmage/cursor-agent-acp-npm

## Adversarial verification

- **confirmed**: ACP status is native: cursor-agent CLI exposes ACP via subcommand agent acp, ships in the same binary as the interactive CLI, no separate package needed. (cursor.com/docs/cli/acp states ACP functionality is built into the same agent binary, no separate process or plugin required beyond spawning agent acp; Zed's zed.dev/acp/agent/cursor independently labels it Native ACP support, launched from the same binary path with argument acp.)
- **confirmed**: Callability verdict yes: a Rust ACP client host such as trogonai's acp-nats-stdio bridge can spawn agent acp as a stdio subprocess like any other ACP-native CLI agent, using newline-delimited JSON-RPC 2.0, forwarding session/request_permission, optionally passing through .cursor/mcp.json, with auth via CURSOR_API_KEY or interactive cursor_login. (cursor.com/docs/cli/acp confirms stdio transport with newline-delimited JSON-RPC 2.0 (stdin requests, stdout responses, stderr logs), authenticate method cursor_login, CURSOR_API_KEY/--api-key as a non-interactive auth path, and support for project-level .cursor/mcp.json; the trogonai repo's acp-nats-stdio crate confirms a real crate whose README describes exactly this stdio-to-NATS bridging role, so the mechanism is architecturally grounded rather than speculative.)
- **confirmed**: Cursor's CLI exposes ACP by running agent acp, stdio JSON-RPC 2.0 with newline-delimited framing, stdin for requests, stdout for responses/notifications, stderr for logs (source: cursor.com/docs/cli/acp). (Directly verified by fetching cursor.com/docs/cli/acp, which states this exact transport and framing.)
- **refuted**: As of October 2025 Cursor staff said on the community forum ACP was not supported and only a feature request (source: forum.cursor.com/t/does-cursor-cli-support-acp/132783). (Fetched the forum thread directly: the Cursor staff member Condor made this statement on September 7, 2025, not October 2025; the substance of the quote is accurate but the month is wrong by about a month.)
- **confirmed**: By March 2026 Cursor had joined the ACP Registry and shipped native ACP support usable from JetBrains IDEs (2025.3.2+, AI Assistant plugin, no JetBrains AI subscription required) and from Zed (source: blog.jetbrains.com March 2026 post). (Fetched blog.jetbrains.com directly: confirms Cursor joined the ACP Registry, requires JetBrains IDE 2025.3.2+ with AI Assistant plugin enabled, and explicitly states no JetBrains AI subscription is needed; Zed native support independently confirmed via zed.dev/acp/agent/cursor.)
- **confirmed**: Zed's ACP agent page for Cursor describes it as Native ACP support with agent/plan/ask modes, session management, MCP server support, and permission-based tool approval, launched via agent acp with the binary path added to agent_servers config (source: zed.dev/acp/agent/cursor). (Fetched zed.dev/acp/agent/cursor directly: confirms the exact phrase Native ACP support, the listed capabilities (agent/plan/ask modes, session management, MCP server support, permission-based tool approval), and the agent_servers config setup using agent acp.)
- **confirmed**: Cursor's documented ACP session lifecycle: initialize, authenticate (methodId cursor_login), session/new or session/load, session/prompt, session/update notifications, session/request_permission, optional session/cancel (source: cursor.com/docs/cli/acp). (Fetched cursor.com/docs/cli/acp directly: the lifecycle sequence matches exactly, including the cursor_login methodId and the optional session/cancel step.)

### Corrections (authoritative where they conflict with the body)

The forum statement about ACP not being supported was made on September 7, 2025 (not October 2025). No other corrections; all other claims and key facts were confirmed against primary sources (cursor.com/docs/cli/acp, forum.cursor.com/t/does-cursor-cli-support-acp/132783, blog.jetbrains.com March 2026 post, zed.dev/acp/agent/cursor), and the internal trogonai repo confirms a real acp-nats-stdio crate matching the architecture described in claim 2, though that crate detail is corroborating context from the private repo rather than something the public ACP/Cursor sources themselves establish.

# OpenCode (sst/opencode, and the anomalyco/opencode fork)

Produced by the ACP product case-study pass (2026-07-30), then adversarially verified; corrections below override the body where they conflict.

## OpenCode: native ACP agent, plus its own parallel client/server model

OpenCode (github.com/sst/opencode, MIT licensed) is the clearest "chose both" case study in this corpus, not a "chose differently" one: it ships a first-party, native ACP agent implementation, while also maintaining its own independent HTTP/OpenAPI client-server protocol for its TUI and SDK clients. It did not reject ACP; it added ACP as one more front door alongside a home-grown one it already had for other reasons.

### ACP status

`opencode acp` starts OpenCode as an ACP-compatible subprocess that speaks JSON-RPC over stdio (https://opencode.ai/docs/acp/). The implementation is native code in the OpenCode monorepo, not a shim: packages/opencode/src/acp holds the ACP logic and packages/opencode/src/cli/cmd/acp.ts wires it into the CLI, importing `AgentSideConnection` and `ndJsonStream` from the official `@agentclientprotocol/sdk` npm package (https://github.com/sst/opencode/blob/dev/packages/opencode/src/cli/cmd/acp.ts). Zed's own ACP agent registry lists OpenCode with the exact launch command `./opencode acp` (https://zed.dev/acp/agent/opencode), and OpenCode's docs describe editor wiring for [Zed](./zed.md), JetBrains (via acp.json), and Neovim plugins (Avante.nvim, CodeCompanion.nvim). OpenCode is agent-side only here; it does not act as an ACP client toward other agents.

### What ACP mode exposes

Nearly the full CLI feature set carries over: built-in file/terminal tools, custom tools, slash commands, MCP servers pulled from opencode.json, AGENTS.md project rules, custom formatters/linters, and the agents/permissions system. The one documented gap is that `/undo` and `/redo` are unsupported over ACP (https://opencode.ai/docs/acp/). Because OpenCode reuses its existing MCP client wiring underneath ACP, MCP server configuration passes through unchanged; there is no separate ACP-specific MCP config surface.

### The parallel native protocol

Independent of ACP, `opencode serve` starts a headless HTTP server (default 127.0.0.1:4096) publishing an OpenAPI 3.1 spec with REST endpoints for sessions, messages, files, and config, optionally protected by HTTP basic auth via OPENCODE_SERVER_PASSWORD (https://opencode.ai/docs/server/). The TUI is simply one client of that server. This is OpenCode's own client/server model, predating and coexisting with ACP support: ACP is the interop protocol for external editors, the OpenAPI server is the protocol for OpenCode's own TUI/SDK clients and any automation that wants full session control rather than an editor-shaped session.

### Auth and headless viability

Credentials are provider API keys, not an ACP-level concept: `opencode auth login` writes to ~/.local/share/opencode/auth.json, with precedence env vars > opencode.json `options.apiKey` > auth store (https://opencode.ai/docs/providers/). No OAuth device flow is required for standard providers, so a headless spawn just needs the right environment variable or an auth.json mounted into the container.

### Channel mapping (Slack/Telegram/etc.)

OpenCode itself does not natively map chat channels into sessions. Third-party projects (opencode-channels, opencode-chat-bridge) sit in front of OpenCode's ACP agent and translate Slack/Discord/Telegram/WhatsApp/Matrix/Mattermost/web messages into ACP sessions, using deterministic per-thread session keys so DMs, group chats, and channels stay isolated (https://github.com/kortix-ai/opencode-channels, https://github.com/ominiverdi/opencode-chat-bridge); see [channel mapping](../channel-mapping.md) for the general pattern. This is exactly the shape trogonai would want for its own non-editor channels: a bridge process outside the agent that owns channel-to-session mapping, with ACP as the uniform transport underneath.

### Design lessons for trogonai

Copy: exposing ACP as a thin, additive surface over an agent's existing tool/session internals, rather than restructuring the agent around ACP, keeps a product's native protocol (here, the OpenAPI server) free to serve richer non-editor clients while ACP handles the editor-shaped subset. Copy: treating channel bridging (Slack/Telegram/etc.) as an external concern layered on top of ACP sessions, not a protocol extension, matches trogonai's own gateway-in-front-of-ACP design. Avoid: OpenCode's two protocols (ACP and its OpenAPI server) currently overlap in unclear ways with no documented policy on which is authoritative for a given deployment; trogonai should keep ACP as the single client boundary and treat any richer internal API as private, not client-facing, to avoid the same ambiguity.

### Sources

- https://opencode.ai/docs/acp/
- https://opencode.ai/docs/server/
- https://opencode.ai/docs/providers/
- https://github.com/sst/opencode/blob/dev/packages/opencode/src/cli/cmd/acp.ts
- https://zed.dev/acp/agent/opencode
- https://zed.dev/docs/ai/external-agents
- https://github.com/sst/opencode/blob/HEAD/LICENSE
- https://www.npmjs.com/package/@agentclientprotocol/sdk
- https://github.com/kortix-ai/opencode-channels
- https://github.com/ominiverdi/opencode-chat-bridge
- https://agentclientprotocol.com/overview/clients
- [session store research](../../session-store/products/opencode.md) (anomalyco/opencode fork, commit 62e4641235d7847dadc60da37cca8a023dd54fc1)

## Adversarial verification

- **confirmed**: 1. ACP status is native: CLI subcommand `opencode acp`, implemented in packages/opencode/src/acp, cli entry packages/opencode/src/cli/cmd/acp.ts, built on @agentclientprotocol/sdk's AgentSideConnection over ndJsonStream on stdio (Checked opencode.ai/docs/acp/ (describes `opencode acp` as JSON-RPC over stdio), the GitHub directory listing for sst/opencode packages/opencode/src/acp (dev branch, contains agent.ts, session.ts, tool.ts, permission.ts, etc.), and the raw contents of packages/opencode/src/cli/cmd/acp.ts, which imports and uses AgentSideConnection and ndJsonStream from @agentclientprotocol/sdk; the npm registry confirms @agentclientprotocol/sdk is a real package published by Zed Industries under the agentclientprotocol/agent-client-protocol GitHub org.)
- **refuted**: 2. Callability verdict yes: trogonai can spawn OpenCode as an ACP agent today via `opencode acp` and acp-nats-stdio, spawning the process and wrapping its stdin/stdout, since OpenCode's agent side matches the wire shape acp-nats-stdio expects from Rust SDK 2.0.0 clients, so the integration point is at the process boundary, not inside OpenCode (Confirmed via the trogonai repo (rsworkspace/crates/acp/acp-nats-stdio) that the crate pins agent-client-protocol = "=2.0.0" and reads/writes its OWN process's stdin/stdout (tokio::io::stdin/stdout wrapped via async_compat), matching the wire-level compatibility claim; however, a workspace-wide search for Command::new/tokio::process::Command in the acp/ crates and across the repo (excluding test/import-check files) found no code that spawns third-party subprocesses and pipes them into the bridge, so trogonai has no existing generic ACP-subprocess-spawning host today; that plumbing (spawn `opencode acp`, connect its stdio to acp-nats-stdio's own stdio) does not exist in the codebase yet and would need to be built, making the 'yes, today' framing an overstatement even though the underlying protocol/version compatibility is real.)
- **confirmed**: 3a. OpenCode implements ACP natively as an agent via `opencode acp`, starting an ACP-compatible subprocess over JSON-RPC via stdio (opencode.ai/docs/acp/) (WebFetch of opencode.ai/docs/acp/ directly states this.)
- **confirmed**: 3b. ACP implementation lives in packages/opencode/src/acp, wired into CLI at packages/opencode/src/cli/cmd/acp.ts, which imports AgentSideConnection and ndJsonStream from @agentclientprotocol/sdk (Verified directory listing via gh api and raw file fetch of acp.ts on the dev branch; note the file also imports @opencode-ai/sdk/v2 and an internal auth module, meaning the command internally involves an HTTP client/server layer in addition to the stdio ACP transport, a nuance not mentioned in the claim but not contradicting it.)
- **confirmed**: 3c. Zed lists OpenCode in its official ACP agent registry with launch command `./opencode acp` (or via ACP Registry auto-install) (zed.dev/acp/agent/opencode) (WebFetch of zed.dev/acp/agent/opencode returned exactly this launch command and mentioned ACP Registry auto-install as an alternative.)
- **confirmed**: 3d. Via ACP, OpenCode exposes built-in tools, custom tools, slash commands, configured MCP servers, AGENTS.md rules, custom formatters/linters, agents/permissions system; /undo and /redo explicitly unsupported over ACP (opencode.ai/docs/acp/) (WebFetch of opencode.ai/docs/acp/ listed all of these capabilities verbatim and explicitly stated /undo and /redo are unsupported via ACP.)
- **confirmed**: 3e. OpenCode also runs a separate native client/server model via `opencode serve`: headless HTTP server default 127.0.0.1:4096, OpenAPI 3.1 spec, session/message/file REST endpoints, optional HTTP basic auth via OPENCODE_SERVER_PASSWORD; TUI is one client of this server (opencode.ai/docs/server/) (WebFetch of opencode.ai/docs/server/ confirmed default host/port 127.0.0.1:4096, an OpenAPI 3.1 spec at /doc, session/message/file/config/LSP/MCP REST endpoints, and HTTP basic auth gated by OPENCODE_SERVER_PASSWORD (with OPENCODE_SERVER_USERNAME defaulting to 'opencode').)

### Corrections (authoritative where they conflict with the body)

Claim 2 should be softened from a flat "yes, today" to a conditional yes: the wire-level compatibility is real (both `opencode acp` and trogonai's acp-nats-stdio speak ACP JSON-RPC via stdio framing, and acp-nats-stdio is pinned to agent-client-protocol =2.0.0, matching the schema version OpenCode's ACP layer targets), but trogonai does not currently contain any component (in acp-nats-stdio, acp-nats, acp-nats-agent, or acp-nats-server) that spawns an arbitrary third-party ACP subprocess like `opencode acp` and pipes its stdio into the bridge; a workspace-wide search for Command::new/tokio::process::Command outside test code found none in the acp crates. Corrected claim: "OpenCode's ACP agent implementation is wire-compatible with trogonai's acp-nats-stdio bridge (matching JSON-RPC/stdio framing and the pinned agent-client-protocol =2.0.0 schema version), but trogonai does not yet have a process-spawning host that connects a third-party ACP subprocess such as opencode acp to acp-nats-stdio; that spawn-and-pipe integration layer would need to be built before this becomes callable end-to-end." Minor nuance on claim 1/3b: packages/opencode/src/cli/cmd/acp.ts additionally imports @opencode-ai/sdk/v2 and an internal auth module, so `opencode acp` is not a pure standalone ACP process; it initializes an internal HTTP client/server relationship alongside the stdio ACP transport to the editor.

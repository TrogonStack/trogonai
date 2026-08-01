# Zed (editor) and the Agent Client Protocol (ACP)

Produced by the ACP product case-study pass (2026-07-30), then adversarially verified; corrections below override the body where they conflict.

## Zed and the Agent Client Protocol: case study for trogonai's ACP client boundary

### 1. ACP status

Zed is the origin and reference **client** implementation of ACP, not a bundled agent. The protocol itself now lives in a community-governed repository, `github.com/agentclientprotocol/agent-client-protocol` (Apache-2.0, 3800+ stars), which used to be `zed-industries/agent-client-protocol` (old path still resolves via GitHub's org-rename redirect). The repo publishes two Rust crates: the low-level `agent-client-protocol-schema` (wire types only, on a fast release cadence: Rust Crate v1.6.0 / Schema v1.20.0 / Schema v2.0.0-alpha.2 all cut 2026-07-21) and the higher-level `agent-client-protocol` runtime crate that provides client and agent runtime APIs. crates.io reports `agent-client-protocol` max_stable_version as **2.0.0** (verified via the crates.io API), which matches the exact version trogonai pins in its `acp-nats*` crates. Zed's own integration is native, built into the editor as "External Agents" (agentclientprotocol.com/overview/introduction).

### 2. Integration wiring

ACP's process model has the editor as JSON-RPC client, agent as JSON-RPC server. Local agents are spawned as stdio subprocess children of Zed; remote/cloud agents over HTTP or WebSocket are explicitly flagged as "work in progress" in the spec (agentclientprotocol.com/overview/introduction). Zed configures agents in `settings.json` under `agent_servers`, e.g. `{"agent_servers": {"my-agent": {"type": "custom", "command": "node", "args": ["index.js", "--acp"], "env": {}}}}` (zed.dev/docs/ai/external-agents). Session lifecycle, permission prompts (`session/request_permission`), filesystem RPCs (`fs/read_text_file`, `fs/write_text_file`), and terminal RPCs (`terminal/create`, `terminal/output`, `terminal/wait_for_exit`, `terminal/kill`, `terminal/release`) are all defined in the shared schema (agentclientprotocol.com/protocol/schema); Zed renders the permission requests as approval UI in its Agent Panel. MCP passthrough happens through `session/new`'s `mcpServers` parameter; Zed's docs state MCP servers configured in Zed "may be forwarded to External Agents over ACP" but do not fully spell out the wire mechanism.

### 3. Channel mapping

Zed itself does not map Slack/Telegram/WhatsApp/voice into agent sessions; it is a desktop editor only. However, the broader ACP ecosystem now includes independent third-party ACP **clients** for messaging platforms, listed directly on the official clients page: Discord, Slack, Telegram, WeChat, QQ, Lark, and Matrix integrations (agentclientprotocol.com/get-started/clients), plus standalone bridges like OpenACP, Sniptail, and telegram-acp-bot that consume the ACP Registry and use platform-native constructs (forum topics, threads, channels); see [channel mapping](../channel-mapping.md) for the general pattern. These are separate open-source projects, not Zed code, but they prove the ACP wire format itself is channel-agnostic, in reasoning: any process willing to speak the JSON-RPC schema can be a client, so trogonai's NATS-based client transport is architecturally equivalent to these bridges.

### 4. Callability from trogonai

Zed cannot be spawned headlessly as an ACP agent today: it is the client role, and a GitHub discussion (#59146) requesting a standalone headless CLI/agent mode for Zed's own agent panel remains open and unresolved, with no committed maintainer roadmap answer. There is no `zed --acp` flag or documented server mode. This means trogonai gains nothing from trying to "call Zed" as an agent; what is directly usable is the shared Rust crate: trogonai's `acp-nats*` crates already consume `agent-client-protocol =2.0.0`, the same SDK Zed uses, so trogonai's client-role code is protocol-compatible with any agent Zed can also spawn ([Claude Code](./claude-code.md), [Gemini CLI](./gemini-cli.md), GitHub Copilot, etc, via their own ACP modes). No auth is needed for the protocol layer itself; auth is delegated entirely to whichever agent process is spawned.

### 5. Design lessons

Copy: the `agent_servers` config shape (name, command, args, env) as a clean, declarative subprocess-spawn contract; treating permission requests and fs/terminal access as first-class RPCs rather than baked-in trust; forwarding MCP server config into the session rather than requiring agents to have their own MCP client wiring. Avoid: Zed's remote/HTTP transport is still explicitly unfinished in the spec, so trogonai's own NATS transport for ACP is genuinely novel territory, not something to backport from Zed's example, and should be validated against the schema's JSON-RPC envelope semantics directly rather than assumed compatible with an HTTP/WebSocket variant Zed has not shipped.

### Sources

- https://agentclientprotocol.com/overview/introduction
- https://agentclientprotocol.com/protocol/schema
- https://agentclientprotocol.com/get-started/clients
- https://zed.dev/acp
- https://zed.dev/docs/ai/external-agents
- https://zed.dev/blog/acp-registry
- https://github.com/agentclientprotocol/agent-client-protocol
- https://github.com/agentclientprotocol/agent-client-protocol/releases
- https://github.com/zed-industries/zed/discussions/59146
- https://crates.io/crates/agent-client-protocol

## Adversarial verification

- **confirmed**: Zed is a native ACP client via built-in External Agents feature, agent_servers setting in settings.json (zed.dev/docs/ai/external-agents shows the exact agent_servers JSON block with type: custom, command, args, env, e.g. spawning node index.js --acp; zed.dev/docs/ai/agent-settings confirms External Agents is a built-in Zed feature.)
- **confirmed**: Protocol repo lives at github.com/agentclientprotocol/agent-client-protocol, renamed from zed-industries/agent-client-protocol (Fetching github.com/zed-industries/agent-client-protocol resolves into the agentclientprotocol org content, npm shows @zed-industries/agent-client-protocol was renamed to @agentclientprotocol/sdk, and search results show the old org path now serving agentclientprotocol content, consistent with a GitHub repo transfer/redirect.)
- **confirmed**: Rust crate agent-client-protocol max stable version is 2.0.0 on crates.io (crates.io API (crates.io/api/v1/crates/agent-client-protocol) returns max_stable_version, max_version, and newest_version all as 2.0.0; trogonai's own Cargo.toml pins agent-client-protocol = "=2.0.0", corroborating.)
- **refuted**: agent-client-protocol-schema crate is at schema-v2.0.0-alpha.2 / stable schema-v1.20.0 as of 2026-07-21 (crates.io API shows the actual agent-client-protocol-schema Rust crate's max_stable_version/newest_version is 1.6.0 with no alpha releases published to crates.io; the schema-v1.20.0 and schema-v2.0.0-alpha.2 tags found on GitHub (both dated 2026-07-21) are release tags for the standalone JSON Schema artifact files (used for cross-language SDK generation), a separate versioning line from the Rust crate on crates.io, not the crate's own version. trogonai's Cargo.toml pins agent-client-protocol-schema = "=1.5.0", further confirming the crate versioning is in the 1.x range distinct from the schema-v1.20.0 tag.)
- **confirmed**: trogonai depends on agent-client-protocol =2.0.0 and has acp-nats* crates consuming the same spec/SDK Zed uses (grep of the trogonai repo's rsworkspace/Cargo.toml shows agent-client-protocol = "=2.0.0", agent-client-protocol-schema = "=1.5.0", agent-client-protocol-http = "=2.0.0", and crates acp-nats, acp-nats-agent, acp-nats-server, acp-nats-stdio depending on them via workspace inheritance.)
- **confirmed**: Zed is a GUI editor acting as ACP client, not an ACP agent server; no zed --acp or documented headless agent-server mode (zed.dev/docs/ai/external-agents and search results describe Zed as hosting agent threads in its Agent Panel (client role) and spawning External Agents as subprocesses; no official docs mention a zed --acp flag or headless server mode.)
- **confirmed**: Only related thread is an open, unresolved GitHub discussion about Zed as a headless/standalone agent (github.com/zed-industries/zed/discussions/59146 'Zed Agent as standalone, headless CLI application' was fetched and found open/unresolved, with the most recent activity being a question about whether the eval_cli crate partially addresses it, and no resolution or official headless mode announced.)
- **confirmed**: ACP is an open standard created by Zed Industries, JSON-RPC 2.0 over stdio, modeled on LSP turning NxM into N+M (agentclientprotocol.com/overview/introduction confirms JSON-RPC over stdio and the NxM-to-N+M framing implicitly (custom work per agent-editor pair vs. universal compatibility); WebSearch results independently and consistently attribute ACP's creation to Zed Industries and the explicit LSP/N+M analogy, corroborating the introduction page's framing.)
- **confirmed**: In ACP, editor/IDE is client and coding agent is server; local agents run as stdio subprocess children; remote/cloud agents over HTTP/WebSocket are work in progress (agentclientprotocol.com/overview/introduction explicitly states local agents run as sub-processes of the code editor communicating via JSON-RPC over stdio, and that remote agents hosted in the cloud communicate over HTTP or WebSocket with full support noted as work in progress.)
- **confirmed**: Zed configures custom external agents via agent_servers block with type: custom, command, args, env, e.g. spawning node index.js --acp (zed.dev/docs/ai/external-agents shows this exact JSON example verbatim.)
- **confirmed**: Zed-configured MCP servers may be forwarded to External Agents over ACP (zed.dev/docs/ai/external-agents states 'Zed-configured MCP servers may be forwarded to External Agents over ACP. External Agents may also read their own native MCP configuration.')
- **confirmed**: ACP schema defines session/request_permission, fs/read_text_file, fs/write_text_file, terminal/create, terminal/output, terminal/wait_for_exit, terminal/kill, terminal/release; session/new carries mcpServers parameter (agentclientprotocol.com/protocol/schema was fetched and confirms all listed method names and that NewSessionRequest includes a required mcpServers field for MCP servers the agent should connect to.)

### Corrections (authoritative where they conflict with the body)

The agent-client-protocol-schema Rust crate on crates.io is at max stable version 1.6.0 (not 1.20.0), with no alpha releases published to crates.io at all. The version strings schema-v1.20.0 and schema-v2.0.0-alpha.2 are GitHub release tags for the standalone JSON Schema artifact files (used for cross-language SDK code generation), a separate versioning line from the Rust crate itself; they should not be cited as the Rust crate's version. Corrected statement: "agent-client-protocol-schema (lower-level Rust crate types, max stable version 1.6.0 on crates.io as of 2026-07-21; note the JSON Schema artifact files carry a separate, higher-numbered versioning line, currently schema-v1.20.0 stable / schema-v2.0.0-alpha.2, used for cross-language SDK generation rather than the Rust crate itself)."

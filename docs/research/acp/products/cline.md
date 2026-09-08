# Cline

Produced by the ACP product case-study pass (2026-07-30), then adversarially verified; corrections below override the body where they conflict.

## Cline as an ACP boundary for trogonai

### 1. ACP status

Cline implements ACP natively, as an agent (not a client), via the `cline` CLI's `--acp` flag. This is not a third-party adapter: it is first-party code inside `cline/cline` (Apache-2.0, github.com/cline/cline), announced as part of "Cline CLI 2.0" (https://cline.bot/blog/introducing-cline-cli-2-0), and Cline appears as a native (non-adapter-tagged) entry in the official ACP agent registry at agentclientprotocol.com/get-started/agents and on Zed's own registry page zed.dev/acp/agent/cline, which gives the exact spawn command `npx cline@3.0.47 --acp`. The feature request traces back to cline/cline Discussion #5994 (opened around September 2025, marked implemented by February 2026). Current published package: `cline` on npm, version 3.0.47 (npmjs.com/package/cline). A separate, older community bridge, Tonksthebear/cline-acp, also exists, connecting to cline-core over gRPC, but it predates or duplicates the now-native `--acp` mode and is not the recommended path.

### 2. Integration wiring

Process model is a stdio subprocess: launch `cline --acp` (or `npx cline@<version> --acp`) and speak newline-delimited JSON-RPC over stdin/stdout. Per a DeepWiki technical writeup of the source (secondary source, not independently re-verified against raw code this session, flagged as inference-grade): stdout is reserved exclusively for the ACP JSON-RPC stream, all logs go to stderr, and a `ClineAgent` class (apps/cli/src/acp/acpAgent.ts) maps ACP's `initialize` / `newSession` / `prompt` / `cancel` methods onto an internal `Controller` per session. Internal `ClineMessage` events are translated into ACP `SessionUpdate` payloads (for example `say:tool` becomes a `tool_call`), and tool calls that need approval carry a `requiresPermission` flag so the ACP client drives the approval UI. Filesystem and terminal access are not performed by Cline directly in this mode; they are brokered through an `ACPHostBridgeClientProvider` back to the ACP client (the editor), which is the same "client owns fs/terminal" pattern ACP was designed around. MCP servers are configured independently of ACP, via `cline mcp` / `.cline/mcp.json`, and per Cline's own docs the full agent (Skills, Hooks, MCP) carries through unchanged when running under `--acp`.

### 3. Channel mapping

No evidence found that Cline maps Slack, Telegram, WhatsApp, web chat, mobile, or voice channels into ACP sessions or any other mechanism. Cline's other surfaces (Cline SDK, Cline Desktop, both actively released as of 2026-07-28/29) are separate products from the ACP agent-server mode and were not found to bridge non-editor channels either.

### 4. Callability from trogonai today

Yes. trogonai can spawn `cline --acp` (or `npx cline@3.0.47 --acp`) as a stdio subprocess exactly like any other ACP agent, and relay it through the existing acp-nats-stdio bridge onto NATS the same way it would bridge any stdio-native ACP peer. Auth is the main external dependency: Cline needs model-provider credentials configured via `cline auth --provider <p> --apikey <key> --modelid <m>` (persisted to `~/.cline/data/settings/providers.json`, relocatable per sandbox via `CLINE_DATA_DIR`, or overridden per-run with `-k/--key`). Headless operation is supported (`-y` for full autonomy with no TUI, `--zen` to run in a background hub per the CLI reference), though the precise interaction of those flags with `--acp` mode specifically was not confirmed in primary sources and should be smoke-tested. License is Apache-2.0, which is compatible with internal hosting. Before production wiring, verify Cline's ACP wire-protocol version against trogonai's pinned Rust SDK `agent-client-protocol =2.0.0` (wire v1); this was not independently confirmed in this research pass.

### 5. Design lessons

Copy: treating fs/terminal as client-owned capabilities brokered through a bridge object, rather than agent-owned, keeps the agent portable across hosts, exactly the separation trogonai wants at its ACP boundary. Copy: keeping MCP server configuration as a concern orthogonal to ACP (project-local `.cline/mcp.json`) rather than folding it into the protocol avoids overloading ACP's scope. Copy: shipping the ACP mode as a flag on the same binary/package that already exists (`--acp`) rather than a separate artifact minimizes version drift between "the agent" and "the ACP-speaking agent." Avoid: Cline gives no answer for non-editor channel fan-out, so trogonai should not expect to inherit any multi-channel story from an ACP-native coding agent; that layer has to be trogonai's own responsibility above the ACP boundary, consistent with treating ACP purely as the client-to-agent leg rather than a general session/channel bus.

### Sources

- https://cline.bot/blog/introducing-cline-cli-2-0
- https://docs.cline.bot/cli/cli-reference
- https://github.com/cline/cline
- https://github.com/cline/cline/discussions/5994
- https://github.com/cline/cline/blob/main/LICENSE
- https://www.npmjs.com/package/cline
- https://agentclientprotocol.com/get-started/agents
- https://zed.dev/acp
- https://zed.dev/acp/agent/cline
- https://deepwiki.com/cline/cline/12.5-agent-client-protocol-(acp) (secondary source, technical detail not independently re-verified against raw source this session)
- https://github.com/Tonksthebear/cline-acp
- https://cline.bot/sdk

## Adversarial verification

- **refuted**: ACP status is native, invoked as `cline --acp` or `npx cline@3.0.47 --acp`, implemented at apps/cli/src/acp/acpAgent.ts (ClineAgent class) in github.com/cline/cline (Apache-2.0) (Fetched the raw file at apps/cli/src/acp/acpAgent.ts in cline/cline (main branch) directly via curl and confirmed via grep: the exported class is `export class AcpAgent implements Agent` (line 76), not `ClineAgent`; the file imports Agent, InitializeRequest, NewSessionRequest, PromptRequest, PROTOCOL_VERSION etc. directly from the official `@agentclientprotocol/sdk` package, confirming native (non-adapter) ACP compliance, but the specific class name asserted in the claim is factually wrong.)
- **unverifiable**: Callability verdict yes: trogonai can spawn Cline as an ACP agent today via `npx cline@3.0.47 --acp` as a stdio subprocess, relayed by trogonai's acp-nats-agent/acp-nats-stdio crates with no additional adapter needed beyond the existing bridge (Confirmed Cline's ACP mode is a native stdio JSON-RPC agent (npm registry: cline@3.0.47, repo git+https://github.com/cline/cline.git, directory apps/cli, license Apache-2.0, author Cline Bot Inc., bin: cline, published 2026-07-28) and confirmed trogonai's clone (origin/main) genuinely contains acp-nats-agent and acp-nats-stdio crates plus ADRs 0020/0022 referencing ACP-over-NATS, so the architectural premise is plausible; confirming that the existing bridge code requires zero additional adapter work for Cline specifically would require reading acp-nats-stdio's implementation in depth, which is beyond a primary-source check on Cline's side, so this narrower sub-claim is unverifiable rather than confirmed or refuted.)
- **confirmed**: The --acp flag turns Cline into an ACP-compliant agent; ACP support shipped in Cline CLI 2.0; Cline is listed as a native (non-adapter) agent on the official ACP registry (cline.bot/blog/introducing-cline-cli-2-0 explicitly states 'The --acp flag turns Cline into an ACP-compliant agent'; agentclientprotocol.com/get-started/agents lists Cline as a bare entry '[Cline](https://cline.bot/)' with no adapter/bridge parenthetical, unlike some other listed agents.)
- **confirmed**: Zed's ACP agent registry page for Cline gives the spawn command npx cline@3.0.47 --acp (Fetched zed.dev/acp/agent/cline directly; page states verbatim 'Start Cline with: npx cline@3.0.47 --acp'.)
- **confirmed**: Current published npm package version is cline@3.0.47; repo licensed Apache License 2.0, Copyright 2026 Cline Bot Inc. (Queried registry.npmjs.org/cline directly: dist-tags.latest is 3.0.47 (published 2026-07-28), license Apache-2.0, author 'Cline Bot Inc.', repository points to git+https://github.com/cline/cline.git (directory apps/cli); fetched github.com/cline/cline/blob/main/LICENSE which reads 'Copyright 2026 Cline Bot Inc.' under Apache License 2.0. Note the npm package name 'cline' was previously an unrelated MIT-licensed CLI library by a different author in versions 0.1.0-0.3.0, so the name was later repurposed/taken over by Cline Bot Inc.; this does not contradict the claim but is a provenance detail worth flagging.)
- **refuted**: Cline works in JetBrains, Zed, Neovim (via CodeCompanion/avante.nvim/agentic.nvim), Emacs, and any ACP-speaking editor; Skills, Hooks, MCP integrations carry over into ACP mode (The CLI 2.0 blog post (cline.bot/blog/introducing-cline-cli-2-0) names Neovim integration only via 'CodeCompanion or avante.nvim' and does not mention 'agentic.nvim'; a separate secondary summary of docs.cline.bot/cli/acp-editor-integrations named 'agentic.nvim or avante.nvim' instead, dropping CodeCompanion, so the two sources disagree with each other and neither matches the claim's exact three-plugin list; a direct fetch of docs.cline.bot/cli/acp-editor-integrations returned HTTP 404, so that specific citation could not be independently confirmed, meaning the claim's list of three Neovim plugins is not supported as stated by a verifiable primary source.)
- **confirmed**: Headless/CI automation via -y flag (full autonomy, no interactive TUI, streams to stdout) documented in CLI 2.0 announcement, and a --zen flag ('start a session that runs in the background hub') per the CLI reference (cline.bot/blog/introducing-cline-cli-2-0 states verbatim 'The -y flag gives Cline full autonomy. No interactive TUI. Everything streams to stdout.'; docs.cline.bot/cli/cli-reference lists '-z, --zen  Start a session that runs in the background hub' verbatim among documented flags.)

### Corrections (authoritative where they conflict with the body)

The ACP bridge class in github.com/cline/cline is named AcpAgent (apps/cli/src/acp/acpAgent.ts, line 76: "export class AcpAgent implements Agent"), not ClineAgent. It imports Agent, InitializeRequest, NewSessionRequest, PromptRequest, PROTOCOL_VERSION and related types directly from the official @agentclientprotocol/sdk npm package, which does substantiate native ACP wire-protocol compliance despite the wrong class name.

The claim about Neovim integration listing "CodeCompanion/avante.nvim/agentic.nvim" cannot be confirmed as written: the CLI 2.0 blog post names only CodeCompanion and avante.nvim (no agentic.nvim), a separate secondary source names agentic.nvim and avante.nvim (no CodeCompanion), and the specific docs.cline.bot/cli/acp-editor-integrations URL cited as the source returns HTTP 404 and could not be directly fetched. State this as "Neovim integration via CodeCompanion or avante.nvim (per the CLI 2.0 announcement); some secondary sources also mention agentic.nvim, but this could not be verified against a live primary source" rather than asserting all three plugins as a settled fact.

The trogonai callability sub-claim (that acp-nats-agent/acp-nats-stdio need no further adapter work to spawn Cline specifically) is architecturally plausible given the crates exist in the current trogonai repo, but was not verified at the implementation level and should be labeled unverified/to-be-prototyped rather than a settled "yes."

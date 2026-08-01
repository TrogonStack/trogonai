# Tier 2 ACP Adoption Profiles

Scope: the Agent Client Protocol at agentclientprotocol.com (Zed's editor agnostic protocol for connecting clients to coding agents), not IBM's unrelated Agent Communication Protocol.

## 1. Neovim: CodeCompanion.nvim

CodeCompanion.nvim, maintained by Oli Morris, is the actual maintained Neovim ACP client (not avante.nvim, which has no ACP integration as of this research). It implements ACP Protocol Version 1 with negotiated versioning, and its own docs describe support for JSON-RPC 2.0, streaming responses, message buffering, authentication, file operations, and session management, including resuming prior sessions via `/resume` when an agent supports `session/list`. Agent-advertised slash commands surface in the chat buffer. Two gaps are explicitly acknowledged: terminal operations and agent plan visualization are not yet implemented. It ships adapters for Claude Code, Codex, Gemini CLI, and OpenCode. This is a mature, actively maintained, general purpose AI plugin that added ACP as one of two integration modes (alongside direct HTTP LLM access), signaling that ACP is being absorbed into existing popular tooling rather than requiring a bespoke client.

## 2. Emacs: agent-shell

agent-shell, by Xenodium, is a native Emacs package built specifically around ACP, implemented on top of `comint-mode` to give a shell-like buffer for talking to agents such as Claude Code and Gemini CLI. The author's own posts frame it as a direct response to wanting ACP support in Emacs, and it includes traffic inspection and replay tooling aimed at making agent-agnostic Emacs workflows easier to build. Started via `M-x agent-shell`, it is a young, single-maintainer, actively developed project rather than a broadly co-maintained one. It signals that ACP is attracting dedicated, protocol-first client authors even in editors far outside the VS Code and JetBrains mainstream, evidence the protocol's design is genuinely editor agnostic in practice, not just in spec.

## 3. Obsidian: Agent Client (obsidian-agent-client)

Obsidian has at least two independent ACP client plugins: `obsidian-agent-client` by RAIT-09 (published to the Obsidian Community Plugins directory) and a separate `obsidian-agent` by WhiskeyJack96. The RAIT-09 plugin is the more visible and documented one: it supports note mentions via `@notename`, image attachments, agent-advertised slash commands, multi-agent switching (Claude Code, Codex, Gemini CLI, custom agents), multiple simultaneous sessions, a floating chat window, and mode/model switching. It is community-maintained, not an official Obsidian feature, but its presence in the plugin marketplace indicates a working, installable release rather than a proof of concept. The existence of two competing implementations for the same host app signals real developer interest in wiring ACP into non-code-editor knowledge tools, extending the protocol's reach beyond IDEs.

## 4. Marimo

Marimo, the reactive Python notebook, ships an experimental ACP integration exposed as an Agent sidebar in the editor, letting agents read and write notebook cells directly through the chat panel. Per marimo's own docs, it supports Claude Code, Gemini, Codex (via the `codex-acp` adapter), and OpenCode agents, with custom agent support described as coming soon. There is also a separate, lighter weight `marimo pair` agent skill path recommended for Codex CLI users that gives full notebook session access without going through raw ACP. Marimo's docs are explicit that "agents are currently experimental and under active development," so this is an early but shipped feature from the core marimo team, not a third party plugin. It signals ACP expanding past text editors into computational notebook environments, a genuinely different client category from IDEs and shells.

## 5. OpenHands

OpenHands supports ACP natively rather than through a bolted-on adapter: running `openhands acp` puts the OpenHands CLI directly on the wire as an ACP agent, communicating over JSON-RPC 2.0 with any ACP-speaking client (Zed, CodeCompanion.nvim, Obsidian's agent-client plugins, JetBrains IDEs, and others), and it supports three confirmation modes to gate tool actions. This was built and is maintained by the OpenHands team itself and announced on their own blog in December 2025, with a follow-up June 2026 post covering the "Agent Canvas" for controlling arbitrary coding agents. It is the clearest example in this set of an agent (as opposed to an editor) treating ACP as a first-class, native output surface, signaling that agent-side adoption is now happening symmetrically alongside client-side adoption, which is the condition needed for the protocol to function as a real interop layer rather than a one-directional integration.

## Sources

- [Agent Client Protocol (ACP) Support - CodeCompanion.nvim](https://codecompanion.olimorris.dev/agent-client-protocol)
- [Configuring ACP Adapters - CodeCompanion.nvim](https://codecompanion.olimorris.dev/configuration/adapters-acp)
- [New in v17.18.0 - Agent Client Protocol in CodeCompanion - GitHub Discussion](https://github.com/olimorris/codecompanion.nvim/discussions/2030)
- [Neovim - ACP Client | Zed](https://zed.dev/acp/editor/neovim)
- [GitHub - xenodium/agent-shell](https://github.com/xenodium/agent-shell)
- [Introducing Emacs agent-shell (powered by ACP) - xenodium.com](https://xenodium.com/introducing-agent-shell)
- [So you want ACP (Agent Client Protocol) for Emacs? - xenodium.com](https://xenodium.com/so-you-want-acp-for-emacs)
- [Agent Client for Obsidian (docs)](https://rait-09.github.io/obsidian-agent-client/)
- [GitHub - RAIT-09/obsidian-agent-client](https://github.com/RAIT-09/obsidian-agent-client)
- [Agent Client - Obsidian Community Plugin](https://community.obsidian.md/plugins/agent-client)
- [GitHub - WhiskeyJack96/obsidian-agent](https://github.com/whiskeyjack96/obsidian-agent)
- [Obsidian - ACP Client | Zed](https://zed.dev/acp/editor/obsidian)
- [Agents - marimo docs](https://docs.marimo.io/guides/editor_features/agents/)
- [marimo - ACP Client | Zed](https://zed.dev/acp/editor/marimo)
- [use-acp demo - marimo-team](https://marimo-team.github.io/use-acp/)
- [How the Community is Driving ACP Forward - Zed's Blog](https://zed.dev/blog/acp-progress-report)
- [OpenHands - ACP Agent | Zed](https://zed.dev/acp/agent/openhands)
- [IDE Integration Overview - OpenHands Docs](https://docs.openhands.dev/openhands/usage/cli/ide/overview)
- [Use AI Agents in Your Favorite Editor through the Agent Client Protocol - OpenHands Blog](https://www.openhands.dev/blog/20251209-use-openhands-in-your-ide-with-acp)
- [Controlling any Coding Agent with the OpenHands Agent Canvas and SDK - OpenHands Blog](https://www.openhands.dev/blog/use-any-coding-agent-in-openhands-with-acp)
- [Clients - Agent Client Protocol](https://agentclientprotocol.com/get-started/clients)

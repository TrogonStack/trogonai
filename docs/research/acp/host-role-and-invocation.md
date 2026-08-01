# ACP Host Role and Invocation Mechanics

Filed from query follow-ups (2026-07-30): "does buzz-agent use buzz-acp to call the underlying runtimes?" and "who actually calls codex, claude, cursor and so on, and how?"

## The role that does the calling

There is no central broker in the ACP ecosystem. The caller is always
whatever process plays the **ACP client host** role: it spawns the agent CLI
as a stdio child, speaks the client side of the protocol over the child's
stdin/stdout, and serves the client-owned callbacks. Each host does its own
spawning:

| Host (the caller) | How agents are configured |
|---|---|
| Zed | `agent_servers` in settings.json: `{"command", "args", "env"}`, exec'd as a child process |
| JetBrains AI Assistant | `~/.jetbrains/acp.json`, agent picker, one-click installs from the ACP Registry |
| buzz-acp (Block) | `BUZZ_ACP_AGENT_COMMAND` / `BUZZ_ACP_AGENT_ARGS`; default `goose acp` |
| Devin Desktop | hosts Devin, Codex, Claude Agent, OpenCode side by side |
| OpenClaw | its `acpx` client component, behind the multi-channel gateway |
| Neovim/Emacs plugins, `acpr`, `acp-cli` | same pattern, smaller scope |
| trogonai | nobody yet; the missing `acp-host` component (see [synthesis](./synthesis.md)) |

## Buzz directionality (common confusion)

`buzz-agent` does NOT call other runtimes. The flow is:

```
Buzz relay (@mentions)
      v
buzz-acp        ACP CLIENT: spawns ONE agent subprocess, drives
                initialize / session/new / session/prompt, respawns on crash
      v
agent child     ACP AGENT: default `goose acp`; swappable for codex-acp,
                claude-agent-acp, or buzz-agent itself
      v
MCP servers     spawned by the agent for actual tool access
```

`buzz-agent` is one interchangeable runtime among those children: Block's
minimal hand-rolled ACP agent (wire v1, no SDK dependency) that talks to the
LLM provider directly via env vars and gets all tools from MCP servers. It
implements no permission/fs/terminal RPCs, which pushes all trust decisions
into opaque MCP servers; the [Buzz dossier](./products/buzz.md)
flags that as the anti-pattern for trogonai's permission-broker design.

## Invocation mechanics (the "how")

1. **Spawn**: host launches the agent binary as a child with credentials as
   env vars, piping stdin/stdout. Known-good commands:
   `gemini --acp` (GEMINI_API_KEY), `npx @agentclientprotocol/claude-agent-acp`
   (ANTHROPIC_API_KEY), `codex-acp`, `goose acp`, `opencode acp`,
   `cline --acp`, `cursor-agent agent acp`, `grok agent stdio`, `devin acp`,
   `hermes acp`, `buzz-agent`.
2. **Handshake**: `initialize` over newline-delimited JSON-RPC 2.0 with
   bidirectional capability negotiation (client: fs/terminal offers; agent:
   loadSession, mcpCapabilities, promptCapabilities).
3. **Session**: `session/new` with cwd and `mcpServers`; agent returns a
   sessionId.
4. **Prompting**: `session/prompt`; agent streams `session/update`
   notifications (message chunks, tool_call, tool_call_update) until the
   prompt response carries a final StopReason.
5. **Callbacks flow backwards**: mid-turn the agent calls the HOST:
   `fs/read_text_file`, `fs/write_text_file`, `terminal/*`, and
   `session/request_permission`. The host serves files, runs terminals, and
   approves or denies. The agent never touches disk directly; the caller
   mediates everything. This is the authz hook point for trogonai.

## Adapters add one hop

For Codex and Claude, the spawned binary is an official adapter, not the
vendor CLI itself:

```
host --stdio/ACP--> claude-agent-acp --in-process--> @anthropic-ai/claude-agent-sdk
host --stdio/ACP--> codex-acp        --wraps------>  @openai/codex engine
```

The adapter owns ACP translation; the vendor SDK runs the agent loop. Gemini,
Goose, OpenCode, Cline, Cursor, Grok, and Devin ship ACP mode inside their own
binary, so no extra hop.

## trogonai consequence

The `acp-host` crate recommended in the [synthesis](./synthesis.md)
takes the seat Zed and buzz-acp occupy: spawn the CLI with secrets injected at
spawn time (ADR 0023; ADR 0032 prohibits secrets in ACP messages), serve the
fs/terminal callbacks against sandboxed workspaces, route
session/request_permission through a policy decision point, and bridge the
session onto NATS.

Per-product invocation, auth, and blockers: see the
[callability matrix](./synthesis.md) and each product file under
`./products/`.

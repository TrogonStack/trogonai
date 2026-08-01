# Buzz (block/buzz)

Produced by the ACP product case-study pass (2026-07-30), then adversarially verified; corrections below override the body where they conflict.

## Buzz (block/buzz) as an ACP integration target for trogonai

Buzz is Block's open-source (Apache-2.0), Nostr-based workspace where humans and AI agents share channels, threads, git repos, and automated workflows (https://github.com/block/buzz, https://block.xyz/inside/introducing-buzz-where-humans-and-agents-work-together). Repo created 2026-03-06, latest desktop release v0.5.2 on 2026-07-29, 17.5k stars, actively pushed (gh repo view). Its relevance to trogonai is narrow but concrete: two of its crates, buzz-agent and buzz-acp, sit directly on the ACP boundary trogonai cares about.

### ACP status

Buzz implements ACP on both sides, natively, without depending on the official `agent-client-protocol` Rust crate. Cargo.toml for both `crates/buzz-agent` and `crates/buzz-acp` show no `agent-client-protocol` dependency; instead the wire protocol is hand-rolled. buzz-agent's own README says plainly: "The full server is hand-rolled in main.rs" and documents a handshake with `protocolVersion: 1`, matching the wire protocol v1 that trogonai's `agent-client-protocol =2.0.0` SDK pin also targets (https://github.com/block/buzz/blob/main/crates/buzz-agent/README.md).

- **buzz-agent** is the ACP agent role: "Minimal, unbreakable ACP-compliant agent. Non-streaming. Tool-calls-as-output." It accepts `initialize`, `session/new` (with `mcpServers`), `session/prompt`, and `session/cancel`, and emits `agent_message_chunk` / `tool_call` / `tool_call_update` updates plus a `stopReason`. It advertises `mcpCapabilities: {http:false, sse:false}` and `loadSession:false`.
- **buzz-acp** is the ACP client/harness role: it spawns an agent subprocess (default `goose acp`), sends it `initialize`/`session/new`/`session/prompt` over stdio, and bridges Buzz relay events into ACP prompts. It explicitly supports goose, codex (via the community `codex-acp` npm adapter), and Claude Code (via `claude-agent-acp`), all "any agent that speaks ACP over stdio" (https://github.com/block/buzz/blob/main/crates/buzz-acp/README.md).

Maturity: both crates ship with real test suites (regression tests, fake-subprocess integration tests) and are described as "Works Today" in the repo's tri-state status system, not experimental.

### Integration wiring

Process model is subprocess-over-stdio throughout, the same shape trogonai's acp-nats-stdio already expects. buzz-acp spawns the agent process, performs the ACP handshake, discovers Buzz channels via a relay REST API, and batches per-channel @mention events into single `session/prompt` calls (at most one prompt in flight per channel). MCP servers are passed as part of `session/new.mcpServers` and buzz-agent spawns each as its own stdio child, merging their tools into one catalog namespaced `server__tool`. There is no ACP-native permission, filesystem, or terminal RPC support in buzz-agent; those concerns are entirely delegated to whatever MCP tool server is configured (e.g. `buzz-dev-mcp`, providing shell and file-edit tools). Session recovery: crashed agent subprocesses are respawned by buzz-acp; relay disconnects reconnect with a `since` filter.

### Channel mapping

Buzz is a native chat/git/workflow platform, not a Slack/Telegram/WhatsApp bridge. It has its own channels, threads, DMs, and (per the repo status) an in-progress mobile client behind `buzz-push-gateway`. Third-party multi-channel platforms like [OpenClaw](./openclaw.md) instead integrate Buzz as one channel among many (https://docs.openclaw.ai/channels), rather than Buzz mapping into external chat apps. Channel-to-session mapping inside Buzz is one ACP session per Buzz channel, keyed by NIP-29 group (`h` tag) identifiers.

### Callability from trogonai today

Because buzz-agent already speaks bare ACP-over-stdio wire v1 with zero framework dependencies, trogonai's `acp-nats-agent`/`acp-nats-stdio` bridge should be able to spawn it exactly like it would spawn any other stdio ACP binary: build it (`cargo build --release -p buzz-agent`) and exec it with provider env vars set, e.g. `BUZZ_AGENT_PROVIDER=anthropic ANTHROPIC_API_KEY=sk-ant-... ANTHROPIC_MODEL=claude-sonnet-4-5 ./target/release/buzz-agent`. The blocker is capability mismatch, not protocol mismatch: buzz-agent has no permission/fs/terminal RPCs and only stdio MCP transport, so trogonai must supply all tool access through spawnable local MCP servers. Running trogonai as the thing buzz-acp spawns is also plausible (set `BUZZ_ACP_AGENT_COMMAND`/`BUZZ_ACP_AGENT_ARGS` to trogonai's agent entrypoint) but would need trogonai to expose a NATS-free, bare-stdio ACP agent binary, likely a thin shim over acp-nats-agent.

### Design lessons for trogonai

Copy: the "hand-roll a minimal ACP server, no SDK, no framework" posture proves ACP's wire surface is small enough (four methods, three update variants) that a from-scratch implementation is tractable and testable with real subprocesses instead of mocks; the explicit capability negotiation (`mcpCapabilities.http:false`) so clients never request unsupported transports; per-session MCP server isolation with a strict child-env whitelist as a security boundary; the tiered "Bring Your Own Harness" model (compiled-in, preset-catalog, user-JSON) as a low-friction way to let operators add new ACP agents without code changes. Avoid: buzz-agent's total absence of ACP permission/fs/terminal RPCs pushes all trust decisions into opaque MCP servers, which trogonai's own permission-broker design should treat as a cautionary contrast rather than a pattern to copy, since it forfeits any protocol-level audit or consent hook for tool use.

### Sources

- https://github.com/block/buzz
- https://github.com/block/buzz/blob/main/ARCHITECTURE.md
- https://github.com/block/buzz/blob/main/AGENTS.md
- https://github.com/block/buzz/blob/main/VISION_AGENT.md
- https://github.com/block/buzz/blob/main/crates/buzz-agent/README.md
- https://github.com/block/buzz/blob/main/crates/buzz-agent/Cargo.toml
- https://github.com/block/buzz/blob/main/crates/buzz-acp/README.md
- https://github.com/block/buzz/blob/main/crates/buzz-acp/Cargo.toml
- https://block.xyz/inside/introducing-buzz-where-humans-and-agents-work-together
- https://agentclientprotocol.com/
- https://github.com/agentclientprotocol/codex-acp
- https://github.com/agentclientprotocol/claude-agent-acp
- https://docs.openclaw.ai/channels
- https://www.devtoolsdaily.com/blog/a-week-with-buzz-coding-agents/

## Adversarial verification

- **confirmed**: ACP status is native: buzz-agent is a hand-rolled ACP agent binary at wire protocolVersion 1 with no agent-client-protocol crate dependency, and buzz-acp is an ACP client/harness that spawns and speaks stdio JSON-RPC to any ACP agent (goose, codex-acp, claude-agent-acp, or buzz-agent itself). (buzz-agent README states protocolVersion 1 and 'The full server is hand-rolled in main.rs'; buzz-agent/Cargo.toml and buzz-acp/Cargo.toml list no agent-client-protocol or acp-named dependency (buzz-agent depends on rmcp for MCP, not ACP); buzz-acp README confirms it spawns goose (default), codex-acp, claude-agent-acp, or any ACP-speaking binary including buzz-agent via BUZZ_ACP_AGENT_COMMAND.)
- **refuted**: Callability verdict is yes: trogonai can host buzz-agent TODAY behind acp-nats-agent via a stdio adapter that spawns buzz-agent as a child process, and conversely buzz-acp could spawn trogonai's acp-nats-agent as a stdio-invocable binary via BUZZ_ACP_AGENT_COMMAND/ARGS. (Inspection of trogonai (origin/main, e79ee7912) shows acp-nats-stdio's main.rs reads/writes its own process stdin/stdout directly (tokio::io::stdin()/stdout()) with no Command::new/tokio::process spawn anywhere in it, and a repo-wide grep for process-spawning code across the workspace found zero hits in any acp crate; acp-nats-agent is a library-only crate (no main.rs, no [[bin]] target in its Cargo.toml), so it is not a stdio-invocable binary buzz-acp could spawn, and there is no 'stdio adapter that spawns' buzz-agent as claimed.)
- **confirmed**: buzz-agent's handshake uses protocolVersion 1, matching the wire version trogonai's acp-nats stack pins. (buzz-agent README initialize response shows protocolVersion:1; trogonai's docs/architecture/acp-conformance.md states 'Wire protocol | v1' as of last review 2026-07-27 (the pinned Rust SDK crate is agent-client-protocol 2.0.0, a separate axis from the wire protocolVersion, which the ACP spec repo confirms has remained 1 across SDK major bumps).)
- **confirmed**: Neither buzz-agent nor buzz-acp depends on the official agent-client-protocol Rust crate; ACP is hand-rolled JSON-RPC over stdio. (Fetched both Cargo.toml files directly; buzz-agent depends on rmcp/tokio/serde/etc with no acp crate, buzz-acp depends on buzz-core/nostr/tokio-tungstenite/etc with no acp crate, and buzz-agent README states the ACP server is hand-rolled in main.rs.)
- **confirmed**: buzz-acp is an ACP harness bridging Buzz relay events to any ACP-speaking agent: goose, codex (via codex-acp), claude code (via claude-agent-acp). (buzz-acp README describes it as an 'ACP harness that connects AI agents to Buzz,' listening for @mentions on the relay and spawning goose (default), codex-acp, or claude-agent-acp as ACP subprocesses.)
- **confirmed**: buzz-agent advertises mcpCapabilities http:false, sse:false and loadSession:false in its ACP initialize response. (buzz-agent README initialize response shows mcpCapabilities:{http:false,sse:false} with 'Transport: stdio only. No HTTP, no SSE' and loadSession:false explicitly stated.)
- **confirmed**: Agent identity in Buzz is a Nostr keypair (NIP-01/NIP-42) independent of platform; agents authenticate to the relay via NIP-42 and act under BUZZ_PRIVATE_KEY. (buzz-acp README states agents authenticate via NIP-42 using BUZZ_PRIVATE_KEY (nsec1...), and the block.xyz post independently confirms the platform-independent-keypair framing ('a cryptographic keypair that belongs to them, not to the platform'), though the blog itself does not cite NIP numbers by name.)

### Corrections (authoritative where they conflict with the body)

Claim 2 (the callability verdict and its invocation mechanism) is refuted and should read: trogonai cannot host buzz-agent today via any existing spawn mechanism, because no component in trogonai's ACP stack (acp-nats-stdio, acp-nats-agent, acp-nats-server, acp-nats) spawns child processes; acp-nats-stdio is itself the stdio-boundary process (it reads/writes its own stdin/stdout to whatever external process spawned it, e.g. an IDE), and acp-nats-agent is a library crate with no binary target at all. A protocol-version match at the wire level (both are protocolVersion 1) is necessary but not sufficient for callability; the missing piece is that trogonai has no process-supervisor/spawner component analogous to buzz-acp's harness. To actually host buzz-agent, trogonai would need to build a new component that spawns `buzz-agent` as a stdio child and bridges its stdin/stdout onto NATS subjects using the acp-nats-agent library as its NATS-side handler; this component does not exist today. Symmetrically, buzz-acp could only spawn a trogonai binary if trogonai first built and shipped a stdio-invocable ACP agent binary (which acp-nats-agent, being library-only, does not provide); no such binary exists in the current trogonai codebase (origin/main at e79ee7912, reviewed 2026-07-30).

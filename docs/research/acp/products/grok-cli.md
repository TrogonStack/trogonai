# Grok Build (xAI's coding agent CLI, github.com/xai-org/grok-build)

Produced by the ACP product case-study pass (2026-07-30), then adversarially verified; corrections below override the body where they conflict.

## Grok Build (xAI) and the Agent Client Protocol

### What it is

Grok Build is xAI's terminal AI coding agent, shipped from the public repo `xai-org/grok-build` (Rust, Apache-2.0, ~23.4k stars, actively pushed) [github.com/xai-org/grok-build](https://github.com/xai-org/grok-build). It runs three ways: an interactive full-screen TUI, headless mode for scripts and CI, and as an ACP agent embedded in editors and other tools [docs.x.ai/build/overview](https://docs.x.ai/build/overview). This is distinct from `superagent-ai/grok-cli`, an unrelated third-party open-source CLI for the Grok API; this dossier is about xAI's own product only.

### ACP status: native

ACP support is native, not an adapter. The crate `xai-acp-lib` inside the grok-build workspace wraps the real upstream `agent-client-protocol` Rust SDK crate (from `agentclientprotocol/rust-sdk`), declared as a workspace dependency with the `unstable` feature enabled and pinned to version 0.10.4 in the workspace root Cargo.toml (github.com/xai-org/grok-build Cargo.toml and xai-acp-lib/Cargo.toml). xAI baked ACP directly into the binary: no separate adapter process or wrapper is needed. This differs from trogonai's own SDK pin of `agent-client-protocol` =2.0.0; both target ACP wire protocol v1, which is versioned independently of SDK crate semver, so the numeric gap is a maturity or feature-surface signal more than a hard wire incompatibility (inference, not independently load-tested here).

### Invocation and process model

Grok is spawned as a stdio subprocess for local integration:

```
grok agent stdio
grok agent --always-approve stdio    # yolo/auto-approve mode, alias --yolo
```

A WebSocket transport also exists for remote clients: `grok agent --always-approve serve --bind 127.0.0.1:2419 --secret <token>`, and `--leader` lets multiple sessions share one backing process (xai-grok-pager/docs/user-guide/15-agent-mode.md). The client (editor, orchestrator) spawns grok, not the reverse. Session lifecycle follows standard ACP shape: `initialize` (protocol version and capability negotiation), `session/new` (cwd, an `mcpServers` array, optional `_meta` flags like `yoloMode`), `session/prompt`, then streamed `session/update` notifications (`agent_message_chunk`, `agent_thought_chunk`, `tool_call`, `tool_call_update`, `plan`). Permission modes are always-approve, auto (heuristic), and ask (default, interactive prompts), settable by flag or per-session `_meta`.

Beyond ACP core, grok layers proprietary extension RPCs under the `x.ai/*` namespace: `x.ai/fs/*` (list, read_file, write_file, exists), `x.ai/terminal/*` (create, kill, output, wait_for_exit), `x.ai/git/*` (status, stage, commit, diffs), `x.ai/session/*` (fork, resolve_local_for_worktree_resume), and `x.ai/auth/*` (get_url, submit_code) for client-mediated OAuth-style login over the RPC channel itself. MCP passthrough is a plain array in `session/new`; AGENTS.md, plugins, hooks, and skills all load automatically ("Bring Your Own MCP").

### Channel mapping

No evidence found of native Slack, Telegram, WhatsApp, or voice channel mapping. The product's multi-surface story is TUI, headless scripting, and ACP for editor and tool embedding; ACP itself is the channel abstraction point, and any chat-platform bridging would be a client built on top speaking ACP to grok, not something xAI ships directly.

### Callability from trogonai today

Yes. trogonai's `acp-nats-stdio` bridge can spawn `grok agent stdio` as a subprocess exactly as it would any other ACP agent, relaying JSON-RPC over the NATS subject binding. Auth for unattended use is `XAI_API_KEY` in the environment; interactive login can round-trip through `x.ai/auth/get_url` and `x.ai/auth/submit_code`. License (Apache-2.0) imposes no embedding constraint. The main integration risk is the SDK version gap (0.10.4+unstable vs trogonai's pinned =2.0.0) and the proprietary `x.ai/*` extensions, which a generic client can safely ignore but won't benefit from without custom handling.

### Design lessons for trogonai

Copy: baking the real upstream ACP crate straight into the agent binary with no adapter tax, a clean stdio-first process model with an optional WebSocket variant for remote or shared use, and building the agent's own durable session log directly on ACP's `SessionId` and `session/update` vocabulary rather than a parallel internal format. Avoid: leaning on `unstable`-gated SDK features and proprietary `x.ai/*` extensions for core functionality, since that couples session and tool semantics to a single vendor's fork of the schema and complicates cross-agent interoperability, exactly the fragmentation ACP exists to prevent.

### Sources

- https://github.com/xai-org/grok-build
- https://docs.x.ai/build/overview
- https://github.com/xai-org/grok-build/blob/main/Cargo.toml
- https://github.com/xai-org/grok-build/blob/main/crates/codegen/xai-acp-lib/Cargo.toml
- https://github.com/xai-org/grok-build/blob/main/crates/codegen/xai-grok-pager/docs/user-guide/15-agent-mode.md
- https://docs.x.ai/build/cli/headless-scripting
- https://agentclientprotocol.com/protocol/overview
- https://github.com/agentclientprotocol/rust-sdk
- https://github.com/superagent-ai/grok-cli

## Adversarial verification

- **confirmed**: ACP status: grok-build's xai-acp-lib wraps the upstream agent-client-protocol Rust crate, workspace-pinned to 0.10.4 with the unstable feature, invoked via `grok agent stdio`, `grok agent serve --bind <addr> --secret <token>`, or `grok agent --leader stdio`, with no separate adapter binary required. (Fetched crates/codegen/xai-acp-lib/Cargo.toml (agent-client-protocol = { workspace = true, features = ["unstable"] }) and root Cargo.toml (version = "0.10.4", features = ["unstable"]); verified 0.10.4 is a real, non-yanked crates.io release of agentclientprotocol/rust-sdk with an unstable feature; verified the three command forms in the official 15-agent-mode.md doc and repo README/docs.x.ai text.)
- **refuted**: Callability verdict is yes: trogonai's acp-nats-stdio bridge can spawn `grok agent stdio` as a child process like any other stdio ACP agent and relay JSON-RPC over acp-nats, with unstable-feature extension RPCs being additive calls a conformant client can ignore if unimplemented. (Read trogonai source at acp-nats-stdio/src/main.rs and acp-nats/src/lib.rs: acp-nats-stdio reads/writes its own process's stdin/stdout and implements the ACP Agent role itself (agent::Bridge, AgentHandler) to be spawned BY a client host, not the reverse; no tokio::process or std::process::Command exists anywhere under the acp crates, so it cannot spawn grok as a child process. Additionally the official ACP extensibility spec (agentclientprotocol.com/protocol/extensibility) states unrecognized ext_method (request-shaped) calls return a JSON-RPC 'Method not found' (-32601) error rather than being silently ignorable; only unrecognized notifications SHOULD be ignored, contradicting the claim's blanket ignorability statement for request-shaped extensions like x.ai/fs/read_file.)
- **confirmed**: Grok Build can be used via interactive TUI, headlessly in scripts/CI, or embedded in editors via ACP (docs.x.ai/build/overview). (Fetched docs.x.ai/build/overview directly; page text reads 'Use it via an interactive TUI, headlessly in scripts or bots, or through the Agent Client Protocol (ACP) in other apps,' matching the claim.)
- **confirmed**: ACP agent mode commands: `grok agent stdio`, `grok agent --always-approve stdio`, `grok agent --always-approve serve --bind 127.0.0.1:2419 --secret <token>`, `--leader` shares a backing process (15-agent-mode.md). (Fetched the raw doc file from github.com/xai-org/grok-build/blob/main/crates/codegen/xai-grok-pager/docs/user-guide/15-agent-mode.md; it shows exactly these command forms and flag descriptions, including --always-approve/--yolo and --leader/--no-leader semantics.)
- **confirmed**: xai-acp-lib's Cargo.toml uses agent-client-protocol as a workspace dependency with the unstable feature, and workspace root pins 0.10.4+unstable, i.e. consumes the real upstream crate rather than a reimplementation. (Fetched both Cargo.toml files directly (raw.githubusercontent.com); confirmed exact lines. Cross-checked crates.io API: agent-client-protocol's repository field points to github.com/agentclientprotocol/rust-sdk (the official Zed-affiliated ACP Rust SDK), confirming this is the genuine upstream crate, and 0.10.4 is a valid unyanked version with an unstable feature matching the claimed feature set.)
- **confirmed**: grok-build is Apache-2.0, public repo, ~23.4k stars, last push 2026-07-29 (GitHub API). (Ran `gh api repos/xai-org/grok-build`: license Apache-2.0, stars 23463 (matches ~23.4k), pushed_at 2026-07-29T17:17:58Z, private=false, archived=false.)
- **confirmed**: Session lifecycle over ACP: initialize then session/new (cwd, mcpServers, _meta) then session/prompt then streamed session/update notifications (agent_message_chunk, agent_thought_chunk, tool_call, tool_call_update, plan) per 15-agent-mode.md. (Fetched the doc directly; it lists initialize -> session/new (with working directory) -> session/prompt -> session/update notifications, and enumerates exactly these five sessionUpdate kinds with matching descriptions.)

### Corrections (authoritative where they conflict with the body)

Claim 2 should read: trogonai does not currently have code that spawns `grok agent stdio` (or any external ACP agent binary) as a child process. The acp-nats-stdio binary implements the ACP Agent role over its own stdin/stdout and bridges to NATS, meaning trogonai is designed to be spawned as an agent by an external ACP client host, not to act as a client host spawning other agents such as grok-build. If trogonai wants to call grok-build as an ACP agent, a new component playing the ACP Client role (one that spawns `grok agent stdio` as a subprocess and speaks JSON-RPC over its stdio) would need to be built; no such component was found in the current trogonai codebase. Separately, the claim that unstable-feature extension RPCs (x.ai/fs/*, x.ai/terminal/*, x.ai/git/*, x.ai/session/*, x.ai/auth/*) are all freely ignorable if unimplemented should be qualified: per the official ACP spec, this is true only for notification-shaped extension calls (ext_notification, SHOULD be ignored if unrecognized); request-shaped extension calls (ext_method) that go unimplemented produce a JSON-RPC "Method not found" (-32601) error rather than being silently skippable, so a client wanting true interoperability must either implement or gracefully handle such errors for any request-shaped x.ai/* extension it does not support.

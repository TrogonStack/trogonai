# OpenClaw

Produced by the ACP product case-study pass (2026-07-30), then adversarially verified; corrections below override the body where they conflict.

## OpenClaw and ACP: a two-sided bridge, not a channel-transport protocol

OpenClaw (github.com/openclaw/openclaw, MIT, TypeScript, ~385k stars, self-described as "Your own personal AI assistant. Any OS. Any Platform.") is a multi-channel personal-assistant Gateway that plays both ACP roles, but only at its internal automation boundary, never as the mechanism that carries WhatsApp/Telegram/Discord traffic into a session.

### ACP status: both agent and client, natively, via two separate surfaces

OpenClaw implements ACP twice. As an ACP server, `openclaw acp` is a CLI bridge that "speaks ACP over stdio for IDEs and forwards prompts to the Gateway over WebSocket" (docs.openclaw.ai/cli/acp), letting Zed or VS Code drive a Gateway session as if it were a normal ACP agent. As an ACP client, any agent config with `runtime: "acp"` makes OpenClaw spawn an external coding harness (Codex, Claude Code, Cursor, Gemini CLI, Copilot, Droid, OpenCode, and others) through the `@openclaw/acpx` runtime plugin (docs.openclaw.ai/tools/acp-agents). The client-side implementation lives in a separate, MIT-licensed, alpha-stage repo, `openclaw/acpx` (npm package `acpx`, package.json version 0.13.0), which depends on `@agentclientprotocol/sdk ^1.3.0`, the TypeScript ACP SDK from the same Zed-originated lineage as trogonai's Rust `agent-client-protocol` crate. acpx's own README warns "the CLI/runtime interfaces are likely to change," so treat the client side as unstable.

### Integration wiring

Both directions use real OS subprocesses over stdio, not HTTP or a cloud API. In server mode, IDEs launch `openclaw acp` as a child process that itself opens a WebSocket to a running Gateway. In client mode, OpenClaw (or acpx directly) launches the target harness (`codex`, `claude`, etc.) as a subprocess: "ACP launches a real external harness process. OpenClaw owns routing, background-task state, delivery, bindings, and policy; the harness owns its provider login, model catalog, filesystem behavior, and native tools" (docs.openclaw.ai/tools/acp-agents). Sessions get Gateway-native keys, `agent:<agentId>:acp:<uuid>` for client-spawned sessions, `acp-bridge:<uuid>` by default for server/bridge mode (overridable with `--session`/`--session-label`). The Gateway is the sole owner of session persistence (SQLite plus JSONL transcripts); IDE and channel clients only query it. Permission requests are resolved headlessly through an explicit two-key policy on the acpx plugin: `permissionMode` (`approve-all` / `approve-reads` / `deny-all`) and `nonInteractivePermissions` (`fail`, the default, throws `PermissionPromptUnavailableError`; or `deny`, which silently declines). MCP passthrough is asymmetric: bridge/server mode explicitly rejects per-session `mcpServers` rather than silently dropping them, while client mode does pass custom `mcpServers` through to the spawned harness and additionally offers two opt-in bridges (`pluginToolsMcpBridge`, `openClawToolsMcpBridge`) to expose OpenClaw's own tools as MCP servers to that harness.

### Channel mapping: the case study answer is "not over ACP"

This is the key finding for trogonai. Non-editor channels (WhatsApp, Telegram, Discord, web chat) map into OpenClaw's native Gateway session model via routing rules (`dmScope`, per-group/per-channel isolation, "most-specific wins" binding), entirely independent of ACP. ACP only enters a channel-bound conversation if that conversation's agent config sets `runtime: "acp"`, at which point the existing channel session forwards its turns into a spawned harness process. So ACP in OpenClaw is an internal add-on execution runtime layered under an already-established channel session, not the transport that creates or carries the channel session itself.

### Callability from trogonai today

Not directly, because of a transport mismatch: OpenClaw's ACP surfaces run over stdio/WebSocket with the TS SDK, while trogonai's boundary is JSON-RPC-over-NATS via `agent-client-protocol` (Rust, pinned 2.0.0). Inference: the practical bridge is trogonai's own `acp-nats-stdio` adapter wrapping `openclaw acp --url ws://<gateway>:18789 --token-file <path> --session agent:<agentId>:main` as the stdio peer, assuming the wire-protocol versions actually negotiate compatibly at `initialize` (unverified here, worth a handshake test given acpx pins SDK `^1.3.0` against trogonai's `=2.0.0`). Auth requirements stack: a Gateway WebSocket token/password for the ACP bridge itself, plus whatever channel credentials the target session needs (Telegram: a bot token via CLI, fully headless; WhatsApp: QR-only login, awkward but not blocking if the QR image is relayed out of band once). License is MIT, no constraint.

### Design lessons for trogonai

Copy: the clean separation of "who owns the session" (always the Gateway) from "who executes the turn" (an interchangeable harness, native or ACP), and the explicit, typed headless-permission policy (`permissionMode` x `nonInteractivePermissions`) as a first-class config surface rather than an implicit default. Avoid: running ACP-spawned processes fully unsandboxed on the host ("OpenClaw's sandbox policy does not wrap ACP harness execution") and forbidding MCP passthrough at the exact layer (bridge/server mode) where an integrator would most expect it to work.

## Sources

- https://docs.openclaw.ai/tools/acp-agents
- https://docs.openclaw.ai/cli/acp
- https://github.com/openclaw/openclaw
- https://github.com/openclaw/openclaw/blob/main/docs/tools/acp-agents-setup.md
- https://github.com/openclaw/acpx
- https://github.com/openclaw/acpx/blob/main/package.json
- https://acpx.sh/install.html
- https://docs.openclaw.ai/concepts/session
- https://docs.openclaw.ai/concepts/agent
- https://docs.openclaw.ai/channels/whatsapp
- https://docs.openclaw.ai/install/docker

## Adversarial verification

- **confirmed**: ACP status "native": openclaw acp (bridge/server mode) ships in openclaw/openclaw as CLI command "openclaw acp" (Raw fetch of github.com/openclaw/openclaw/blob/main/docs/cli/acp.md confirms 'openclaw acp' is a real CLI command that speaks ACP over stdio for IDEs and forwards to the Gateway over WebSocket; it is explicitly framed as OpenClaw acting as the ACP server.)
- **confirmed**: acpx is github.com/openclaw/acpx, npm package "acpx" v0.13.0, alpha, MIT, depends on @agentclientprotocol/sdk ^1.3.0 (npm registry JSON (registry.npmjs.org/acpx) shows dist-tags latest = 0.13.0, license MIT; repo package.json (raw.githubusercontent.com) shows the same version, MIT license, and dependency '@agentclientprotocol/sdk: ^1.3.0'; README states alpha status verbatim ('acpx is in alpha and the CLI/runtime interfaces are likely to change').)
- **confirmed**: acpx is invoked internally via the @openclaw/acpx runtime plugin when an agent config sets runtime: "acp" (Raw docs/tools/acp-agents.md and docs/tools/acp-agents-setup.md both instruct 'openclaw plugins install @openclaw/acpx' and describe runtime:"acp" as the acpx dispatch path, though note the scoped npm name '@openclaw/acpx' itself 404s on npm per openclaw/openclaw issues #32967/#32380 -- the actually-published package is unscoped 'acpx'; this is an inconsistency inside OpenClaw's own docs/tooling, not a fabrication in the claim.)
- **confirmed**: Callability verdict "partial": OpenClaw's ACP surfaces speak stdio/WebSocket not NATS, and use TypeScript SDK not the Rust agent-client-protocol crate that trogonai's acp-nats-* crates wrap (Verified directly in the trogonai repo: rsworkspace/Cargo.toml pins agent-client-protocol = "=2.0.0" (Rust crate) used by acp-nats, acp-nats-stdio, acp-nats-agent, acp-nats-server; OpenClaw's own docs confirm openclaw acp speaks stdio (IDE side) and WebSocket (Gateway side) using the TypeScript @agentclientprotocol/sdk, a different implementation of the same protocol.)
- **confirmed**: The nearest real bridge: run `openclaw acp --url ws://<gateway-host>:18789 --token-file <path> --session agent:<agentId>:main` as a stdio subprocess, fronted by trogonai's existing acp-nats-stdio adapter (docs/cli/acp.md documents exactly this flag set (`--url`, `--token-file`, `--session agent:<id>:<label>`) verbatim in its Usage and Zed-setup examples; acp-nats-stdio's own CLI help text ('ACP stdio to NATS bridge for agent-client protocol') and README architecture diagram (IDE <-> stdio <-> acp-nats-stdio <-> NATS <-> Backend) confirm it is built to wrap exactly this kind of stdio ACP peer.)
- **refuted**: trogonai could appear as an ACP agent to OpenClaw via an 'acp-nats-agent-fronted' stdio shim forwarding to trogonai over NATS (No crate named 'acp-nats-agent-fronted' exists in the trogonai repo; the actual crate is 'acp-nats-agent' (README: 'Server-side framework for building ACP agents over NATS') -- the underlying architectural claim (a NATS-backed agent-side crate could front trogonai as an ACP agent) is directionally correct, but the specific crate name cited does not exist verbatim in the repo.)
- **confirmed**: OpenClaw plays both ACP roles: openclaw acp exposes Gateway sessions as ACP server; acpx-backed runtime="acp" path makes OpenClaw an ACP client spawning external harnesses (Codex, Claude Code, Cursor, Gemini CLI, Copilot, etc.) (Raw docs/tools/acp-agents.md lists these exact harness ids (claude, codex, copilot, cursor, gemini, droid, opencode, etc.) under the acpx backend, and contrasts this with 'openclaw acp' bridge mode in a routing table on the same page.)
- **confirmed**: acpx is MIT-licensed, marked alpha with 'the CLI/runtime interfaces are likely to change', depends on @agentclientprotocol/sdk ^1.3.0, same protocol lineage from Zed Industries (Confirmed via package.json (MIT, dependency, alpha wording from README) and independently confirmed ACP was created and open-sourced by Zed Industries (multiple independent secondary sources: tessl.io, Medium, Ry Walker research), consistent with framing ACP as 'Zed's protocol'.)
- **confirmed**: ACP sessions get Gateway session keys `agent:<agentId>:acp:<uuid>`; bridge mode defaults to `acp-bridge:<uuid>` unless `--session`/`--session-label` pins a session; Gateway owns all session state (SQLite + JSONL) (Raw docs/tools/acp-agents.md comparison table shows `Session key | agent:<agentId>:acp:<uuid>` for acpx/client-mode ACP sessions; raw docs/cli/acp.md states bridge mode defaults to an isolated `acp-bridge:<uuid>` session unless you override the key or label; docs/concepts/session confirms 'All session state is owned by the gateway' stored in SQLite (openclaw-agent.sqlite) plus JSONL transcripts.)
- **confirmed**: Headless permission auto-resolution: permissionMode = approve-all | approve-reads | deny-all, nonInteractivePermissions = fail (default, throws PermissionPromptUnavailableError) | deny (Raw docs/tools/acp-agents-setup.md contains this exact table verbatim: permissionMode values approve-all/approve-reads/deny-all, and nonInteractivePermissions fail (default, 'Abort the session with PermissionPromptUnavailableError') / deny ('Silently deny the permission and continue').)
- **confirmed**: MCP passthrough: bridge (server) mode rejects per-session mcpServers with an explicit error rather than silently ignoring; agent (client) mode passes custom mcpServers through; two opt-in bridges pluginToolsMcpBridge and openClawToolsMcpBridge expose OpenClaw's own tools as MCP servers to spawned harnesses (Raw docs/cli/acp.md states verbatim: 'Per-session mcpServers are not supported in bridge mode. If an ACP client sends them during newSession or loadSession, the bridge returns a clear error instead of silently ignoring them'; raw docs/tools/acp-agents-setup.md confirms both pluginToolsMcpBridge and openClawToolsMcpBridge by name, their config keys, and that 'Custom mcpServers still work as before' in the acpx/agent-mode path.)

### Corrections (authoritative where they conflict with the body)

Correction to claim 2 / the callability verdict's bridge suggestion: the crate that would let trogonai appear as an ACP agent to OpenClaw is named "acp-nats-agent" in the actual trogonai repo (rsworkspace/crates/acp/acp-nats-agent, described as "Server-side framework for building ACP agents over NATS"), not "acp-nats-agent-fronted" as stated in the claim. The architectural direction is correct (a NATS-backed agent-side shim using acp-nats-agent could front trogonai and be exec'd by OpenClaw's acpx config as an external harness), but the specific crate name cited does not exist verbatim.

Correction to the licensing claim: the body calls `openclaw/openclaw` "MIT"
without qualification, but GitHub's repository metadata reports its license
as `NOASSERTION` / "Other". Both are defensible and the discrepancy is
mechanical, not substantive: the repo's `LICENSE` is the standard MIT text
verbatim, followed by one appended sentence ("Third-party notices for
incorporated or adapted code are recorded in THIRD_PARTY_NOTICES.md"), and
that addition is enough to defeat GitHub's exact-match license detector. The
grant itself is MIT, so the body's "License is MIT, no constraint" conclusion
holds, but anyone re-deriving the license from GitHub's API or the repo
sidebar will see "Other" rather than "MIT". By contrast, `openclaw/acpx` is
detected as MIT outright, so the acpx licensing claims elsewhere in this
dossier need no such qualification.

Minor note on claim 1: OpenClaw's own official docs instruct "openclaw plugins install @openclaw/acpx" (scoped package name), but that scoped name 404s on npm per openclaw/openclaw GitHub issues #32967 and #32380; the artifact actually published to npm is the unscoped "acpx" package, currently at v0.13.0. This is an inconsistency inside OpenClaw's own documentation/tooling, not an inaccuracy introduced by the claim under test, since the claim's npm package name and version are both independently verified correct against the npm registry.

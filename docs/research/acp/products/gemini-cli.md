# Gemini CLI (google-gemini/gemini-cli)

Produced by the ACP product case-study pass (2026-07-30), then adversarially verified; corrections below override the body where they conflict.

## Gemini CLI as an ACP agent: case study

Gemini CLI (`google-gemini/gemini-cli`, Apache-2.0, npm `@google/gemini-cli`) is Google's open-source terminal AI coding agent and, notably, the launch partner Zed used to introduce the Agent Client Protocol (ACP) to the wider ecosystem (https://zed.dev/blog/bring-your-own-agent-to-zed). It implements ACP natively, in-process, as the **agent** side of the protocol; it is not a client and not a third-party adapter.

### ACP status

ACP mode is invoked with `gemini --acp` (add `--debug` for verbose ACP tracing) (https://github.com/google-gemini/gemini-cli/blob/main/docs/cli/acp-mode.md). Historically the flag was `--experimental-acp`; GitHub issue #22782 and discussion #15885 confirm both flag names have existed during a transition period, and current docs only document `--acp`, so treat `--experimental-acp` as the deprecated predecessor (https://github.com/google-gemini/gemini-cli/issues/22782). Communication is JSON-RPC 2.0 over stdio, matching the ACP spec's transport. Implemented methods: `initialize`, `authenticate`, `newSession`, `loadSession`, `prompt`, `cancel`, `setSessionMode` (approval/permission levels), and `unstable_setSessionModel` (the `unstable_` prefix signals this part of the surface is still moving) (https://github.com/google-gemini/gemini-cli/blob/main/docs/cli/acp-mode.md).

### Integration wiring

Process model: the ACP client (an editor/IDE) spawns `gemini --acp` as a stdio subprocess; gemini-cli never listens on a socket in this mode. Zed's own gemini-cli agent page describes this explicitly as spawning the same CLI binary as a subprocess speaking ACP (https://zed.dev/acp/agent/gemini-cli). Filesystem access is proxied: gemini-cli does not touch the workspace filesystem itself in ACP mode, it issues fs/terminal RPCs back to the client, which enforces what paths are visible (https://github.com/google-gemini/gemini-cli/blob/main/docs/cli/acp-mode.md). MCP passthrough happens during `initialize`: the client advertises its own MCP server, gemini-cli connects to it and folds the discovered tools into the same tool-calling loop it uses for its native MCP client support. Session lifecycle (new/load/resume) rides on ACP's `newSession`/`loadSession`, but the underlying durable transcript is gemini-cli's own local append-only JSONL log under `~/.gemini/tmp/<projectShortId>/chats/`, single-writer, no multi-host coordination; see the [session store research](../../session-store/products/gemini-cli.md) for detail.

### Channel mapping

No evidence of channel mapping (Slack, Telegram, WhatsApp, web chat, mobile, voice) into ACP sessions. ACP-mode clients documented are exclusively editors/IDEs: Zed and a JetBrains (IntelliJ/PyCharm/WebStorm) plugin (https://medium.com/google-cloud/how-to-integrate-gemini-cli-with-intellij-idea-using-acp-0ce55aa623ab). A separate mechanism, the Agent2Agent (A2A) protocol via the `@google/gemini-cli-a2a-server` package, lets gemini-cli run as an A2A server for remote subagent delegation with its own GCS-backed persistence, but A2A is a distinct Google-led protocol, not ACP, and is not documented as a channel bridge either (https://github.com/google-gemini/gemini-cli/tree/main/packages/a2a-server); see [ACP vs A2A](../acp-vs-a2a.md) for the broader comparison.

### Callability from trogonai today

Invocation: `gemini --acp` as a stdio subprocess is directly compatible with trogonai's `acp-nats-stdio` bridging pattern (wrap a stdio ACP agent onto NATS subjects). Auth: `GEMINI_API_KEY` env var or Vertex AI Application Default Credentials/service-account JSON are headless-viable; OAuth "Login with Google" requires an interactive browser and is not headless unless a token is pre-cached (https://github.com/google-gemini/gemini-cli/blob/main/docs/get-started/authentication.mdx). License is Apache-2.0, no constraint on embedding. Main integration effort for trogonai: implementing the client side of fs/terminal RPCs (gemini-cli expects the host to serve files, not read them itself) and pinning an exact release given the nightly/weekly cadence and the `unstable_` method in the ACP surface.

### Design lessons for trogonai

Copy: proxied filesystem access as the default trust boundary (agent never touches disk directly, host mediates every read/write) is a strong sandboxing pattern trogonai's ACP host side should also enforce for third-party agents. Avoid: don't couple session identity to filesystem paths the way gemini-cli's `projectHash`/`projectShortId` scheme does (their own docs show drift between the two, requiring migration logic); trogonai should keep session keys independent of workspace path. Also avoid shipping unstable ACP extension methods without a clear versioning signal beyond a naming prefix.

### Sources

- https://github.com/google-gemini/gemini-cli/blob/main/docs/cli/acp-mode.md
- https://zed.dev/acp/agent/gemini-cli
- https://zed.dev/blog/bring-your-own-agent-to-zed
- https://zed.dev/acp
- https://github.com/google-gemini/gemini-cli/issues/22782
- https://github.com/google-gemini/gemini-cli/discussions/15885
- https://github.com/google-gemini/gemini-cli/blob/main/docs/get-started/authentication.mdx
- https://github.com/google-gemini/gemini-cli/blob/main/packages/cli/package.json
- https://github.com/google-gemini/gemini-cli/tree/main/packages/a2a-server
- https://www.npmjs.com/package/@google/gemini-cli-a2a-server
- https://medium.com/google-cloud/how-to-integrate-gemini-cli-with-intellij-idea-using-acp-0ce55aa623ab
- https://github.com/google-gemini/gemini-cli (repo root, Apache-2.0 license)

## Adversarial verification

- **confirmed**: 1. ACP status is native via `gemini --acp` flag, npm package @google/gemini-cli, current nightly 0.55.0-nightly.20260729.g3499c84f7, superseding the older --experimental-acp flag per docs/cli/acp-mode.md. (docs/cli/acp-mode.md on the google-gemini/gemini-cli GitHub repo confirms `gemini --acp` is the documented invocation; `npm view @google/gemini-cli versions` confirms 0.55.0-nightly.20260729.g3499c84f7 was published (though it is one build behind the true current nightly, 0.55.0-nightly.20260730.gdc859e8e4, as of 2026-07-30); GitHub code search on main shows `--experimental-acp` still referenced in packages/cli/src/config/config.ts and its test file, consistent with a transition rather than a hard removal, matching the claim's careful wording.)
- **refuted**: 2. A Rust ACP client host (trogonai's acp-nats-agent/acp-nats-server crates) can spawn `gemini --acp` as a stdio subprocess and bridge its stdin/stdout JSON-RPC frames onto NATS via the acp-nats-stdio crate, the same way Zed spawns it. (Reading acp-nats-stdio/src/main.rs, acp-nats-agent/README.md, and acp-nats-server/README.md in the trogonai repo (origin/main) shows the opposite architecture: acp-nats-stdio binds to its OWN process's tokio::io::stdin()/stdout() (i.e. it is the thing an IDE spawns and talks to over stdio, occupying the position gemini-cli itself would occupy), acp-nats-agent is a library for writing your own agent business logic behind NATS (no subprocess spawning), and acp-nats-server bridges NATS to HTTP/WebSocket, not stdio subprocesses; a repo-wide grep for `tokio::process`/`std::process::Command` and for 'gemini' across the acp crates and ADRs returns zero relevant hits, so no code exists to spawn gemini-cli as a child process and bridge it onto NATS as claimed.)
- **confirmed**: 3a. ACP mode invoked with `gemini --acp` (optionally --debug); JSON-RPC 2.0 over stdio between client and gemini-cli as the ACP agent. (docs/cli/acp-mode.md states ACP mode activates via `gemini --acp` with `--debug` available for diagnostics, and describes JSON-RPC 2.0 over stdio.)
- **confirmed**: 3b. --experimental-acp was the older flag, superseded by --acp; GitHub issue #22782 and discussion #15885 show both flag forms existed during transition with hang/debugging issues. (Issue #22782 ('--experimental-acp and/or --acp flags hang indefinitely') was fetched directly and its title/body/comments confirm both flag names were in concurrent use with reported hangs, including an IntelliJ user using --experimental-acp as late as gemini-cli 0.33.x.)
- **confirmed**: 3c. ACP methods implemented: initialize, authenticate, newSession, loadSession, prompt, cancel, setSessionMode, and unstable_setSessionModel. (docs/cli/acp-mode.md lists exactly these methods with matching descriptions (initialize for MCP registration, setSessionMode for approval levels, unstable_setSessionModel for mid-session model switching).)
- **confirmed**: 3d. File system access is proxied: gemini-cli does not touch the filesystem directly in ACP mode but issues fs/terminal RPCs back to the client. (docs/cli/acp-mode.md states the agent only has access to files the client has explicitly allowed, matching the proxied-fs claim.)
- **confirmed**: 3e. MCP passthrough: the ACP client implements its own MCP server advertised during initialize; gemini-cli connects to it and exposes its tools to the model, layered on native MCP client support. (docs/cli/acp-mode.md describes the client running an MCP server, sharing connection details at initialize, and gemini-cli discovering and exposing those tools to the model.)

### Corrections (authoritative where they conflict with the body)

Claim 2's mechanism is incorrect and should be corrected to: trogonai's acp-nats-stdio crate is architecturally positioned to be spawned BY an IDE/host (like Zed spawns gemini-cli today) and occupies the agent-facing stdio slot itself, bridging that stdio session onto NATS so the real agent logic (built against acp-nats-agent, or reached via acp-nats-server over HTTP/WebSocket) can live behind NATS. None of the three crates spawn an external stdio ACP agent binary such as gemini-cli as a child process; no tokio::process::Command or std::process::Command usage for this purpose exists anywhere in the workspace, and no file in the acp crates or ADRs mentions gemini. To actually make gemini-cli callable from a Rust ACP client host in the way described, trogonai would need a new adapter crate that spawns `gemini --acp` as a child process and forwards its stdio frames onto acp-nats-stdio's own stdio interface (or directly onto NATS) -- this does not exist today. The auth-provisioning claim (GEMINI_API_KEY or Vertex AI ADC/service-account JSON as headless-friendly options, versus OAuth requiring an interactive browser) is plausible based on general gemini-cli auth documentation but was not directly re-verified against a primary source in this pass and should be treated as unverified rather than confirmed.

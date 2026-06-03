# ACP — Agent Communication Protocol

ACP is a TrogonStack-native protocol that sits next to MCP and A2A in
the agentgateway release. It targets developer-tool integrations
(`acp-nats-stdio`, IDE plugins, terminal agents) where the
client-server shape is a long-lived session with prompt/cancel,
filesystem, and terminal proxy semantics rather than the request/reply
shape of MCP or the task-graph shape of A2A.

## Why ACP

MCP standardizes **tool invocation** between clients and servers.
A2A standardizes **agent-to-agent** task delegation. Neither
addresses the session-oriented interactive flow developer tools need:

- Session resume across reconnects.
- Mode / config / model overrides per session.
- Filesystem proxy and terminal proxy as a first-class protocol
  surface.
- Permission requests routed back to a human in the loop.

ACP fills that gap on the same NATS substrate.

## Crate layout

| Crate | Purpose |
|-------|---------|
| `acp-nats` | Wire types, subjects, JetStream provisioning. |
| `acp-nats-agent` | Agent-side connection (handles client→agent calls). |
| `acp-nats-server` | NATS server binary that bridges stdio or HTTP. |
| `acp-nats-stdio` | Stdio transport for IDE/CLI clients. |

## Subject grammar

```
acp.agent.session.new               # client → agent
acp.session.<session_id>.agent.*    # client → agent (session-scoped)
acp.session.<session_id>.client.*   # agent → client (session-scoped)
```

Session IDs are opaque to the substrate; the gateway does not pin
their format. The `acp-session-id` header echoes the session ID on
the wire for correlation.

## v0.1 scope

- Session lifecycle: `initialize`, `new`, `load`, `fork`, `resume`,
  `close`.
- Per-session config: `set_session_config_option`, `set_session_mode`,
  `set_session_model`.
- Auth: `authenticate`, `logout`.
- Prompt / cancel.
- Filesystem proxy (`fs_read_text_file`, `fs_write_text_file`).
- Terminal proxy (`terminal_{create,kill,output,release,wait_for_exit}`).
- Permission requests, session updates, extension methods.
- JetStream provisioning.

## What ACP does not do

- ACP is **not** a substitute for A2A. ACP clients are first-party
  developer tools; A2A is the multi-org agent federation surface.
- ACP sessions are **subject-only** today; there is no `acp-sessions`
  KV bucket. Cross-pod session resume requires either a sticky
  consumer (queue group with `replicas=1`) or a future v0.2 KV-backed
  session model.
- ACP is **not** in the v0.1 LLM-gateway or inference-routing scope
  (those are ADR 0017 Option A non-goals).

## Where to look next

- Wire types and subjects: `rsworkspace/crates/acp-nats/src/`.
- Server entry point: `rsworkspace/crates/acp-nats-server/src/main.rs`.
- Stdio bridge: `rsworkspace/crates/acp-nats-stdio/`.

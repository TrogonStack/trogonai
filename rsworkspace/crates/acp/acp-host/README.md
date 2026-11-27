# acp-host

Runs any [ACP](https://agentclientprotocol.com/) agent CLI as a child process
and serves it on the `acp-nats` subjects, so every NATS-side ACP client (the
channel bridges, `acp-nats-stdio`, `acp-nats-server`) can reach it. This is the
"ACP client host" from `docs/research/acp/decision-record.md`.

```text
NATS client ─ACP over acp-nats─▶ acp-host ─ACP over stdio─▶ agent CLI (child)
            ◀── client.session.update / request_permission ──┘
```

## Swapping agents is configuration

| Agent | `ACP_HOST_AGENT` |
| --- | --- |
| fx.rs | `trogonai-agent acp` |
| Claude Code | `npx -y @zed-industries/claude-agent-acp` |
| Codex | `npx -y @zed-industries/codex-acp` |
| Pi | `npx -y pi-acp` |
| Gemini CLI | `gemini --acp` |

The value is a command line (leading `NAME=value` words become child env) or
the Zed `agent_servers` JSON shape, `{"command":…,"args":[…],"env":{…}}`. The
child also inherits the host's environment, so keep provider keys there rather
than in the spec. `Debug` output of the spec prints env names, never values.

## Behavior

- **One client per acp-host.** A host serves exactly one NATS-side client.
  `acp-nats` delivers the child's `session/update` and
  `session/request_permission` to every client listening on the host's prefix,
  without filtering by session, so two clients on one prefix would each see
  the other's turns and could answer the other's permission requests. The
  first `initialize` claims the host for its `clientInfo.name`; a later
  `initialize` from a different name is refused with a JSON-RPC
  `-32600 Invalid Request` naming both clients. The same client initializing
  again (after it restarts, or after the child restarts) is accepted. The
  claim lasts until the host exits. It is a guard against misconfiguration,
  not access control: the host cannot see who sends a NATS request, so two
  instances of one client look alike, and a client that never initializes is
  not checked. Keep clients apart for real with NATS permissions per prefix.
  See [Running one host per client](#running-one-host-per-client).
- **One child, initialized once.** The host initializes the child when it
  starts and answers the client's `initialize` with that reply, so a client
  that reconnects does not make the child initialize twice on one connection.
  The child is initialized with default client capabilities, so it uses its
  own filesystem and terminal tools.
- **Callbacks are routed per session.** `session/update`,
  `session/request_permission`, `fs/*` and `terminal/*` from the child go to
  `{prefix}.v1.session.{id}.client.*` for the session they name.
- **Bridge signals stay on NATS.** The client-side bridge publishes
  `{prefix}.v1.session.{id}.agent.ext.ready` when a session is set up. That
  subject sits under the `agent.ext.>` space the host serves, but it is the
  bridge's own signal to its clients, so the host never forwards it to the
  child as a `_ready` notification.
- **Relaunch on exit.** If the child exits, it is relaunched after 1s, doubling
  per short-lived run up to 30s, and reset after a run that lasted 60s.
  Whether a session survives a relaunch depends on the agent. fx keeps
  sessions on disk and reloads one when the client sends `session/load`, so
  its sessions carry on; an agent that keeps sessions only in memory has never
  heard of the old id, and the client has to open a new session. For about
  1.4s after the child dies (until the new child is initialized and serving),
  calls fail: first with "transport closed" from the dead connection, then
  with NATS "no responders" while nothing serves the prefix. Clients should
  retry rather than treat either as fatal.
- **Streams.** The host creates only `COMMANDS` and `RESPONSES`, which the
  JetStream prompt path needs. `GLOBAL`, `GLOBAL_EXT` and `CLIENT_OPS` capture
  subjects that are also used for core request/reply; if any of them exists,
  JetStream answers those requests with its publish ack first and the
  requester takes it as the reply. See `src/streams.rs`.

## Configuration

| Variable | Meaning |
| --- | --- |
| `ACP_HOST_AGENT` | Required. The agent to launch. |
| `ACP_PREFIX` | ACP subject prefix (default `acp`). Give each host, and the one client it serves, its own. |
| `NATS_URL`, `NATS_*` | NATS connection, as for the other ACP binaries. |

## Running locally

```bash
nats-server -js
NATS_URL=nats://127.0.0.1:4222 NATS_TLS_DISABLED=true \
  ACP_HOST_AGENT="trogonai-agent acp" cargo run -p acp-host
```

Then point any ACP client at `acp-nats-stdio` with the same `NATS_URL`.

## Running one host per client

Give every client its own host on its own ACP prefix, and point the client at
the same prefix:

```bash
# Discord bridge and its agent
ACP_PREFIX=acp-discord ACP_HOST_AGENT="trogonai-agent acp" cargo run -p acp-host
ACP_PREFIX=acp-discord cargo run -p channel-bridge-discord

# An editor through acp-nats-stdio, with its own agent
ACP_PREFIX=acp-zed ACP_HOST_AGENT="trogonai-agent acp" cargo run -p acp-host
ACP_PREFIX=acp-zed cargo run -p acp-nats-stdio
```

Each host creates its own `COMMANDS` and `RESPONSES` streams under its prefix,
so the hosts share nothing but the NATS server.

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

- **One child, many sessions.** The host initializes the child once, then
  answers every client's `initialize` with that reply (ACP has a client
  initialize once per connection; here many clients share one connection).
  The child is initialized with default client capabilities, so it uses its
  own filesystem and terminal tools.
- **Callbacks are routed per session.** `session/update`,
  `session/request_permission`, `fs/*` and `terminal/*` from the child go to
  `{prefix}.v1.session.{id}.client.*` for the session they name.
- **Relaunch on exit.** If the child exits, it is relaunched after 1s, doubling
  per short-lived run up to 30s, and reset after a run that lasted 60s.
  Sessions do not survive a relaunch; clients see the session as lost and open
  a new one.
- **Streams.** The host creates only `COMMANDS` and `RESPONSES`, which the
  JetStream prompt path needs. `GLOBAL`, `GLOBAL_EXT` and `CLIENT_OPS` capture
  subjects that are also used for core request/reply; if any of them exists,
  JetStream answers those requests with its publish ack first and the
  requester takes it as the reply. See `src/streams.rs`.

## Configuration

| Variable | Meaning |
| --- | --- |
| `ACP_HOST_AGENT` | Required. The agent to launch. |
| `ACP_PREFIX` | ACP subject prefix (default `acp`). |
| `NATS_URL`, `NATS_*` | NATS connection, as for the other ACP binaries. |

## Running locally

```bash
nats-server -js
NATS_URL=nats://127.0.0.1:4222 NATS_TLS_DISABLED=true \
  ACP_HOST_AGENT="trogonai-agent acp" cargo run -p acp-host
```

Then point any ACP client at `acp-nats-stdio` with the same `NATS_URL`.

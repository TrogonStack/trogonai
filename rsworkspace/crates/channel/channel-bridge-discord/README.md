# channel-bridge-discord

Carries Discord messages to an ACP agent and the agent's replies back, the
same way `channel-bridge-telegram` does for Telegram. Everything
channel-neutral comes from `trogon-channel` (identity, conversations in
JetStream KV) and `channel-acp-port` (the ACP port and reply buffer); this
binary only knows Discord. See `docs/architecture/multi-channel-agent-routing.md`.

```text
Discord ─gateway─▶ trogon-gateway ─▶ DISCORD stream (discord.message_create)
        ─▶ channel-bridge-discord ─ACP over acp-nats─▶ agent (e.g. acp-host)
Discord ◀─ HTTP API ── channel-bridge-discord ◀── session/update
```

## What it answers

- **Direct messages** from a linked user. A DM is keyed on the user's id (not
  the DM channel's), so a user seeded with `CHANNEL_SEED_DISCORD_USERS` is
  recognized there; the reply goes to the DM channel.
- **Guild messages that mention the bot**, keyed on the channel. As with
  Telegram groups, a channel needs its own linked endpoint in the KV store.
- Never messages from bots, including its own.

The bot's own user id is read from Discord at boot, so mentions and self
messages are recognized from the token alone.

Replies ping nobody: `@everyone`, `@here`, role and user mentions in agent
output still render but notify no one, since the agent's text is untrusted. A
long reply is split into messages of at most 2000 UTF-16 code units, which fits
Discord's limit however it counts.

## Configuration

| Variable | Default | Meaning |
| --- | --- | --- |
| `DISCORD_BOT_TOKEN` | required | Bot token, for the HTTP API. |
| `DISCORD_BOT_ACCOUNT` | `bot` | Account half of every endpoint. |
| `DISCORD_INBOUND_STREAM` | `DISCORD` | Stream the gateway's Discord source provisions. |
| `DISCORD_INBOUND_SUBJECT` | `discord.message_create` | Filter on that stream. |
| `CHANNEL_SEED_DISCORD_USERS` | none | Comma-separated Discord user ids to link at boot. |
| `CHANNEL_PREFIX`, `CHANNEL_AGENT_ID`, `CHANNEL_AGENT_CWD`, `CHANNEL_NEW_SESSION_TRIGGERS`, `ACP_PREFIX`, `NATS_*` | | As for `channel-bridge-telegram`. |

The gateway's default intents exclude privileged ones. Discord still sends
message content for DMs and for messages that mention the bot, which is all
this bridge reads; enable `MESSAGE_CONTENT` only if that changes.

## Known gaps

The same as the Telegram bridge's (no streaming of tool calls or plans,
permission requests refused, one message at a time), plus: the processing
pipeline is a copy of Telegram's with the platform calls swapped. Making it
generic over the platform would let both bridges share it, the way they now
share the ACP port.

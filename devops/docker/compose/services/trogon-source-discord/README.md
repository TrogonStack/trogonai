# Receiving Discord Events Locally

`trogon-source-discord` is the inbound pipe for Discord events into NATS
JetStream. It runs in one of two mutually exclusive modes, controlled by
`DISCORD_MODE`:

| Mode | Transport | Events | Requires |
|---|---|---|---|
| `gateway` | WebSocket (you connect to Discord) | Everything — messages, reactions, members, voice, interactions, etc. | `DISCORD_BOT_TOKEN` |
| `webhook` | HTTP POST (Discord connects to you) | Interactions only — slash commands, buttons, modals, autocomplete | `DISCORD_PUBLIC_KEY` |

## Prerequisites

- Docker Compose
- A [Discord Application](https://discord.com/developers/applications)
- An [ngrok](https://ngrok.com) account (free tier, only needed for `webhook` mode)

## Gateway mode

### 1. Get your bot token

1. Go to **Discord Developer Portal → Applications → your app → Bot**
2. Copy the **Token**

### 2. Start the stack

```bash
DISCORD_MODE=gateway \
DISCORD_BOT_TOKEN=<your-bot-token> \
docker compose up
```

This connects to the Discord Gateway and publishes every event to NATS on
`discord.{event_name}` subjects (e.g. `discord.message_create`,
`discord.guild_member_add`).

## Webhook mode

### 1. Get your public key

1. Go to **Discord Developer Portal → Applications → your app → General Information**
2. Copy the **Public Key** (hex string)

### 2. Start the stack

```bash
DISCORD_MODE=webhook \
DISCORD_PUBLIC_KEY=<your-hex-public-key> \
docker compose --profile dev up
```

This starts NATS, the webhook receiver, and ngrok. Find the public tunnel URL
in the ngrok container logs:

```bash
docker compose logs ngrok
```

### 3. Configure your Discord Application

1. Go to **Discord Developer Portal → Applications → your app → General Information**
2. Set **Interactions Endpoint URL** to `https://<ngrok-url>/webhook`
3. Discord will send a PING to verify the endpoint — the receiver handles this
   automatically

## Verify

Subscribe to NATS to see events flowing:

```bash
nats sub -s nats://nats.trogonai.orb.local:4222 "discord.>"
```

Without `--profile dev`, ngrok is excluded and only the core services start.

## Environment variables

| Variable | Required | Default | Description |
|---|---|---|---|
| `DISCORD_MODE` | yes | — | `gateway` or `webhook` |
| `DISCORD_BOT_TOKEN` | gateway mode | — | Bot token for Gateway WebSocket |
| `DISCORD_PUBLIC_KEY` | webhook mode | — | Ed25519 public key (hex) for Interactions |
| `DISCORD_GATEWAY_INTENTS` | no | see code | Comma-separated gateway intents |
| `NGROK_AUTHTOKEN` | webhook + dev | — | ngrok auth token |
| `DISCORD_WEBHOOK_PORT` | no | `8080` | HTTP port for the webhook receiver |
| `DISCORD_SUBJECT_PREFIX` | no | `discord` | NATS subject prefix |
| `DISCORD_STREAM_NAME` | no | `DISCORD` | JetStream stream name |
| `DISCORD_STREAM_MAX_AGE_SECS` | no | `604800` | Max age in seconds for JetStream messages (7 days) |
| `DISCORD_NATS_ACK_TIMEOUT_SECS` | no | `10` | NATS publish ack timeout in seconds |
| `DISCORD_NATS_REQUEST_TIMEOUT_SECS` | no | `2` | NATS request-reply timeout for autocomplete in seconds |
| `DISCORD_MAX_BODY_SIZE` | no | `4194304` | Max webhook body size in bytes (4 MB) |
| `RUST_LOG` | no | `info` | Log level |

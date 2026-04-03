# Receiving Slack Events Locally

## Prerequisites

- Docker Compose
- A Slack App with Event Subscriptions enabled
- An [ngrok](https://ngrok.com) account (free tier works)

## 1. Create a Slack App

1. Go to [api.slack.com/apps](https://api.slack.com/apps) and click **Create New App**
2. Choose **From scratch**, pick a name and workspace
3. Under **Basic Information → App Credentials**, copy the **Signing Secret**

## 2. Start the stack

```bash
docker compose --profile dev up
```

This starts NATS, the Slack webhook receiver, and ngrok. Find the public tunnel
URL in the ngrok container logs:

```bash
docker compose logs ngrok
```

Look for the `slack` tunnel URL (e.g. `https://def456.ngrok-free.app`).

## 3. Enable Event Subscriptions

1. In your Slack App settings, go to **Event Subscriptions**
2. Toggle **Enable Events** to On
3. Set the **Request URL** to `https://<ngrok-url>/webhook`
4. Slack will send a `url_verification` challenge — the server responds
   automatically
5. Under **Subscribe to bot events**, add the events you need:
   - `message.channels` — public channels
   - `message.groups` — private channels
   - `message.im` — direct messages
   - `app_mention` — @mentions anywhere
6. Click **Save Changes**

## 4. Enable Interactivity (optional)

1. Go to **Interactivity & Shortcuts**
2. Toggle **Interactivity** to On
3. Set the **Request URL** to the same `https://<ngrok-url>/webhook`
4. Click **Save Changes**

Block actions, modal submissions, and shortcuts will be published to NATS on
`slack.interaction.{type}` subjects (e.g. `slack.interaction.block_actions`).

## 5. Register Slash Commands (optional)

1. Go to **Slash Commands** and click **Create New Command**
2. Set the **Request URL** to the same `https://<ngrok-url>/webhook`
3. Fill in the command name, description, and usage hint
4. Click **Save**

Slash commands will be published to NATS on `slack.command.{command_name}`
subjects (e.g. `slack.command.trogon`).

## 6. Install the app to your workspace

1. Go to **OAuth & Permissions**
2. Under **Scopes → Bot Token Scopes**, ensure you have the scopes required
   by the events you subscribed to
3. Click **Install to Workspace** and authorize

## 7. Verify

Send a message in a channel where the app is installed. You should see:

- The webhook receiver log the incoming event
- The event published to NATS on `slack.event.message`

You can inspect NATS with:

```bash
nats sub -s nats://nats.trogonai.orb.local:4222 "slack.>"
```

Without `--profile dev`, ngrok is excluded and only the core services start.

## NATS subject mapping

| Slack payload | NATS subject | `X-Slack-Payload-Kind` header |
|---|---|---|
| Events API (`event_callback`) | `slack.event.{event.type}` | `event` |
| Interactions (block actions, modals) | `slack.interaction.{type}` | `interaction` |
| Slash commands | `slack.command.{command_name}` | `command` |

## Environment variables

| Variable | Required | Default | Description |
|---|---|---|---|
| `SLACK_SIGNING_SECRET` | yes | — | Slack app signing secret |
| `NGROK_AUTHTOKEN` | yes (dev profile) | — | ngrok auth token |
| `SLACK_WEBHOOK_PORT` | no | `3000` | HTTP port for the webhook receiver |
| `SLACK_SUBJECT_PREFIX` | no | `slack` | NATS subject prefix |
| `SLACK_STREAM_NAME` | no | `SLACK` | JetStream stream name |
| `SLACK_STREAM_MAX_AGE_SECS` | no | `604800` | Max message age in seconds (default 7 days) |
| `SLACK_NATS_ACK_TIMEOUT_SECS` | no | `10` | NATS acknowledgement timeout in seconds |
| `SLACK_MAX_BODY_SIZE` | no | `1048576` | Max HTTP request body size in bytes (default 1 MB) |
| `SLACK_TIMESTAMP_MAX_DRIFT_SECS` | no | `300` | Max allowed clock drift in seconds (default 5 min) |
| `RUST_LOG` | no | `info` | Log level |

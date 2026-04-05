# Receiving Linear Webhooks Locally

## Prerequisites

- Docker Compose
- A [Linear](https://linear.app) workspace with admin access
- An [ngrok](https://ngrok.com) account (free tier works)

## 1. Get your ngrok tunnel URL

Start only ngrok and NATS to obtain a public URL before configuring Linear:

```bash
docker compose --profile dev up ngrok nats
```

Find the tunnel URL in the ngrok container logs:

```bash
docker compose logs ngrok
```

Look for the `linear` tunnel URL (e.g. `https://abc123.ngrok-free.app`).

## 2. Configure your Linear webhook

1. Go to **Settings → API → Webhooks**
2. Click **New webhook**
3. Set:
   - **URL**: `https://<ngrok-url>/webhook`
4. Select the resource types you want to receive events for
5. Save
6. Copy the **signing secret** that Linear generates

## 3. Start the webhook receiver

Set `LINEAR_WEBHOOK_SECRET` in your `.env` to the signing secret from step 2,
then bring up the full stack:

```bash
docker compose --profile dev up
```

## 4. Verify

Trigger an event in Linear (e.g. create an issue). You should see:

- The webhook receiver log the incoming event
- The event published to NATS on `linear.{type}.{action}`

You can inspect NATS with:

```bash
nats sub -s nats://nats.trogonai.orb.local:4222 "linear.>"
```

Without `--profile dev`, ngrok is excluded and only the core services start.

## Environment variables

| Variable | Required | Default | Description |
|---|---|---|---|
| `LINEAR_WEBHOOK_SECRET` | yes | — | Signing secret from Linear's webhook settings |
| `NGROK_AUTHTOKEN` | yes (dev profile) | — | ngrok auth token |
| `LINEAR_WEBHOOK_PORT` | no | `8080` | HTTP port for the webhook receiver |
| `LINEAR_SUBJECT_PREFIX` | no | `linear` | NATS subject prefix |
| `LINEAR_STREAM_NAME` | no | `LINEAR` | JetStream stream name |
| `LINEAR_STREAM_MAX_AGE_SECS` | no | `604800` | Max message age in seconds (7 days) |
| `LINEAR_WEBHOOK_TIMESTAMP_TOLERANCE_SECS` | no | `60` | Replay-attack window in seconds (0 to disable) |
| `LINEAR_NATS_ACK_TIMEOUT_MS` | no | `10000` | JetStream ACK timeout in milliseconds |
| `NATS_URL` | no | `localhost:4222` | NATS server URL(s) |
| `RUST_LOG` | no | `info` | Log level |

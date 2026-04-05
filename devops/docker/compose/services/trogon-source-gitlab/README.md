# Receiving GitLab Webhooks Locally

## Prerequisites

- Docker Compose
- A [GitLab](https://gitlab.com) project with maintainer access (or a self-hosted instance)
- An [ngrok](https://ngrok.com) account (free tier works)

## 1. Get your ngrok tunnel URL

Start only ngrok and NATS to obtain a public URL before configuring GitLab:

```bash
docker compose --profile dev up ngrok nats
```

Find the tunnel URL in the ngrok container logs:

```bash
docker compose logs ngrok
```

Look for the `gitlab` tunnel URL (e.g. `https://abc123.ngrok-free.app`).

## 2. Configure your GitLab webhook

1. Go to **Settings → Webhooks** in your GitLab project
2. Click **Add new webhook**
3. Set:
   - **URL**: `https://<ngrok-url>/webhook`
   - **Secret token**: a secret of your choice (you will use this as `GITLAB_WEBHOOK_SECRET`)
4. Select the trigger events you want to receive (e.g. Push events, Merge request events)
5. Click **Add webhook**

## 3. Start the webhook receiver

Set `GITLAB_WEBHOOK_SECRET` in your `.env` to the secret token from step 2,
then bring up the full stack:

```bash
docker compose --profile dev up
```

## 4. Verify

Trigger an event in GitLab (e.g. push a commit or open a merge request). You should see:

- The webhook receiver log the incoming event
- The event published to NATS on `gitlab.{event}`

You can inspect NATS with:

```bash
nats sub -s nats://nats.trogonai.orb.local:4222 "gitlab.>"
```

Without `--profile dev`, ngrok is excluded and only the core services start.

## Environment variables

| Variable | Required | Default | Description |
|---|---|---|---|
| `GITLAB_WEBHOOK_SECRET` | yes | — | Secret token configured in GitLab's webhook settings |
| `NGROK_AUTHTOKEN` | yes (dev profile) | — | ngrok auth token |
| `GITLAB_WEBHOOK_PORT` | no | `8080` | HTTP port for the webhook receiver |
| `GITLAB_SUBJECT_PREFIX` | no | `gitlab` | NATS subject prefix (must be valid NATS token) |
| `GITLAB_STREAM_NAME` | no | `GITLAB` | JetStream stream name (must be valid NATS token) |
| `GITLAB_STREAM_MAX_AGE_SECS` | no | `604800` | Max message age in seconds (7 days) |
| `GITLAB_NATS_ACK_TIMEOUT_MS` | no | `10000` | JetStream ACK timeout in milliseconds |
| `GITLAB_MAX_BODY_SIZE` | no | `26214400` | Maximum webhook body size in bytes (25 MB) |
| `NATS_URL` | no | `localhost:4222` | NATS server URL(s) |
| `RUST_LOG` | no | `info` | Log level |

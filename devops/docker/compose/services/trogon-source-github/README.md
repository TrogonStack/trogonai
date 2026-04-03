# Receiving GitHub Webhooks Locally

## Prerequisites

- Docker Compose
- A GitHub App (org or personal) — the app's webhook delivers events from every
  repo where it's installed, so you configure the URL once
- An [ngrok](https://ngrok.com) account (free tier works)

## 1. Generate a webhook secret

```bash
openssl rand -hex 32
```

Save the output — you'll use it in both GitHub and the local stack.

## 2. Start the stack

```bash
docker compose --profile dev up
```

This starts NATS, the webhook receiver, and ngrok. Find the public tunnel URL
in the ngrok container logs:

```bash
docker compose logs ngrok
```

Look for the `github` tunnel URL (e.g. `https://abc123.ngrok-free.app`).

## 3. Configure your GitHub App

1. Go to **Settings → Developer settings → GitHub Apps**
2. Select your app (or create a new one)
3. Under **Webhook**:
   - **Webhook URL**: `https://<ngrok-url>/webhook`
   - **Webhook secret**: the secret you generated in step 1
4. Under **Permissions & events**, subscribe to the events you need
5. Install the app on the repositories or organization you want to receive
   events from

## 4. Verify

Trigger an event in a repository where the app is installed (e.g. push a
commit). You should see:

- The webhook receiver log the incoming event
- The event published to NATS on `github.{event}`

You can inspect NATS with:

```bash
nats sub -s nats://nats.trogonai.orb.local:4222 "github.>"
```

Without `--profile dev`, ngrok is excluded and only the core services start.

## Environment variables

| Variable | Required | Default | Description |
|---|---|---|---|
| `GITHUB_WEBHOOK_SECRET` | yes | — | HMAC-SHA256 secret (must match GitHub App) |
| `NGROK_AUTHTOKEN` | yes (dev profile) | — | ngrok auth token |
| `GITHUB_WEBHOOK_PORT` | no | `8080` | HTTP port for the webhook receiver |
| `GITHUB_SUBJECT_PREFIX` | no | `github` | NATS subject prefix |
| `GITHUB_STREAM_NAME` | no | `GITHUB` | JetStream stream name |
| `RUST_LOG` | no | `info` | Log level |

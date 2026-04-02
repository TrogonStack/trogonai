# Receiving GitHub Webhooks Locally

## Prerequisites

- Docker Compose
- A GitHub App (org or personal) — the app's webhook delivers events from every
  repo where it's installed, so you configure the URL once

## 1. Generate a webhook secret

```bash
openssl rand -hex 32
```

Save the output — you'll use it in both GitHub and the local stack.

## 2. Create a smee.io channel

Go to [smee.io](https://smee.io) and click **Start a new channel**. Copy the
channel URL (e.g. `https://smee.io/abc123`).

## 3. Configure your GitHub App

1. Go to **Settings → Developer settings → GitHub Apps**
2. Select your app (or create a new one)
3. Under **Webhook**:
   - **Webhook URL**: your smee.io channel URL
   - **Webhook secret**: the secret you generated in step 1
4. Under **Permissions & events**, subscribe to the events you need
5. Install the app on the repositories or organization you want to receive
   events from

## 4. Start the stack

```bash
SMEE_URL=https://smee.io/abc123 \
GITHUB_WEBHOOK_SECRET=<secret-from-step-1> \
docker compose --profile dev up
```

This starts NATS, the webhook receiver, and the smee client. The smee client
connects to your channel and forwards events to the webhook receiver.

Without `--profile dev`, the smee client is excluded and only the core services
start.

## 5. Verify

Trigger an event in a repository where the app is installed (e.g. push a
commit). You should see:

- The event appear on your smee.io channel page
- The smee client forward it to the webhook receiver
- The webhook receiver publish it to NATS on `github.{event}`

## Environment variables

| Variable | Required | Default | Description |
|---|---|---|---|
| `GITHUB_WEBHOOK_SECRET` | yes | — | HMAC-SHA256 secret (must match GitHub App) |
| `SMEE_URL` | yes (dev profile) | — | smee.io channel URL |
| `GITHUB_WEBHOOK_PORT` | no | `8080` | HTTP port for the webhook receiver |
| `GITHUB_SUBJECT_PREFIX` | no | `github` | NATS subject prefix |
| `GITHUB_STREAM_NAME` | no | `GITHUB` | JetStream stream name |
| `RUST_LOG` | no | `info` | Log level |

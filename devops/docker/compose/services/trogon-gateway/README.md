# Receiving Source Events Locally

`trogon-gateway` is the unified inbound pipe for platform events into NATS
JetStream. It runs all configured sources in a single process behind a single
HTTP server.

All webhook sources share one port (default `8080`) and are routed by path
prefix:

| Source | Webhook path | Required env var |
|---|---|---|
| GitHub | `/github/webhook` | `TROGON_SOURCE_GITHUB_WEBHOOK_SECRET` |
| Discord | `/discord/webhook` | `TROGON_SOURCE_DISCORD_MODE` |
| Slack | `/slack/webhook` | `TROGON_SOURCE_SLACK_SIGNING_SECRET` |
| Telegram | `/telegram/webhook` | `TROGON_SOURCE_TELEGRAM_WEBHOOK_SECRET` |
| GitLab | `/gitlab/webhook` | `TROGON_SOURCE_GITLAB_WEBHOOK_SECRET` |
| incident.io | `/incidentio/webhook` | `TROGON_SOURCE_INCIDENTIO_SIGNING_SECRET` |
| Linear | `/linear/webhook` | `TROGON_SOURCE_LINEAR_WEBHOOK_SECRET` |

The gateway port is configured via `TROGON_GATEWAY_PORT` (default `8080`).
Liveness and readiness probes are available at `GET /-/liveness` and `GET /-/readiness`.

## Prerequisites

- Docker Compose
- Credentials for the source(s) you want to enable
- An [ngrok](https://ngrok.com) account (free tier, only for webhook sources
  that need a public URL)

## Quick start

All commands below run from the compose directory (`devops/docker/compose`):

```bash
cd devops/docker/compose
cp .env.example .env
# edit .env — only set secrets for sources you want to run
docker compose up
```

## Discord modes

Discord supports two mutually exclusive modes via `TROGON_SOURCE_DISCORD_MODE`:

| Mode | Transport | Events | Requires |
|---|---|---|---|
| `gateway` | WebSocket (you connect to Discord) | Everything | `TROGON_SOURCE_DISCORD_BOT_TOKEN` |
| `webhook` | HTTP POST (Discord connects to you) | Interactions only | `TROGON_SOURCE_DISCORD_PUBLIC_KEY` |

### Gateway mode

```bash
TROGON_SOURCE_DISCORD_MODE=gateway \
TROGON_SOURCE_DISCORD_BOT_TOKEN=<token> \
docker compose up
```

### Webhook mode

```bash
TROGON_SOURCE_DISCORD_MODE=webhook \
TROGON_SOURCE_DISCORD_PUBLIC_KEY=<hex-key> \
docker compose --profile dev up
```

Find the ngrok tunnel URL in `docker compose logs ngrok`, then set it as
your Discord application's Interactions Endpoint URL (append `/discord/webhook`).

## incident.io webhooks

incident.io uses Svix-style webhook signing. Set `TROGON_SOURCE_INCIDENTIO_SIGNING_SECRET`
to the `whsec_...` signing secret from incident.io and configure the webhook
endpoint as `/incidentio/webhook`.

incident.io does not guarantee ordered delivery, and private incident events may
contain only resource IDs rather than full objects. The gateway forwards the raw
verified payload to NATS and leaves any enrichment or reordering to downstream
consumers.

## Exposing webhooks with ngrok

```bash
docker compose --profile dev up
```

This starts ngrok alongside the gateway. Check `docker compose logs ngrok`
for the public URL. Append the source prefix path when configuring each
platform's webhook settings (e.g. `https://<ngrok-url>/github/webhook`).

## Verify

Subscribe to NATS to see events flowing:

```bash
nats sub -s nats://nats.trogonai.orb.local:4222 ">"
```

## Environment variables

See `devops/docker/compose/.env.example` for the full list of configurable
env vars per source. All env vars use the `TROGON_SOURCE_<SOURCE>_` prefix.

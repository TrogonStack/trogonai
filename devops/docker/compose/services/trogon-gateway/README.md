# Receiving Source Events Locally

`trogon-gateway` is the unified inbound pipe for platform events into NATS
JetStream. It runs all configured sources in a single process behind a single
HTTP server, plus the optional Discord gateway runner.

All webhook sources share one port (default `8080`) and are routed by path
prefix:

| Source | Webhook path | Required env var |
|---|---|---|
| GitHub | `/github/webhook` | `TROGON_SOURCE_GITHUB_WEBHOOK_SECRET` |
| Slack | `/slack/webhook` | `TROGON_SOURCE_SLACK_SIGNING_SECRET` |
| Telegram | `/telegram/webhook` | `TROGON_SOURCE_TELEGRAM_WEBHOOK_SECRET` |
| Twitter/X | `/twitter/webhook` | `TROGON_SOURCE_TWITTER_CONSUMER_SECRET` |
| GitLab | `/gitlab/webhook` | `TROGON_SOURCE_GITLAB_WEBHOOK_SECRET` |
| incident.io | `/incidentio/webhook` | `TROGON_SOURCE_INCIDENTIO_SIGNING_SECRET` |
| Linear | `/linear/webhook` | `TROGON_SOURCE_LINEAR_WEBHOOK_SECRET` |
| Notion | `/notion/webhook` | `TROGON_SOURCE_NOTION_VERIFICATION_TOKEN` |
| Sentry | `/sentry/webhook` | `TROGON_SOURCE_SENTRY_CLIENT_SECRET` |

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

## Discord gateway

```bash
TROGON_SOURCE_DISCORD_BOT_TOKEN=<token> \
docker compose up
```

Discord does not use the HTTP ingress or ngrok. It opens an outbound WebSocket
connection to Discord and publishes every gateway event to NATS.

## Telegram webhooks

Set `TROGON_SOURCE_TELEGRAM_WEBHOOK_SECRET` before starting the gateway. To let
the gateway register the webhook on startup, also set
`TROGON_SOURCE_TELEGRAM_WEBHOOK_REGISTRATION_MODE=startup`,
`TROGON_SOURCE_TELEGRAM_BOT_TOKEN`, and
`TROGON_SOURCE_TELEGRAM_PUBLIC_WEBHOOK_URL`.
The URL must be the public HTTPS endpoint for `/telegram/webhook`.

In `manual` mode, the gateway only serves the local webhook receiver and the
Telegram webhook must be configured separately. In `startup` mode, the bot token
and public webhook URL are required. Registration failures are logged and do not
block the gateway from serving configured sources.

## incident.io webhooks

incident.io uses Svix-style webhook signing. Set `TROGON_SOURCE_INCIDENTIO_SIGNING_SECRET`
to the `whsec_...` signing secret from incident.io and configure the webhook
endpoint as `/incidentio/webhook`.

incident.io does not guarantee ordered delivery, and private incident events may
contain only resource IDs rather than full objects. The gateway forwards the raw
verified payload to NATS and leaves any enrichment or reordering to downstream
consumers.

## Twitter/X webhooks

X uses the app consumer secret for both the `GET /twitter/webhook` CRC challenge
response and the `POST /twitter/webhook` `x-twitter-webhooks-signature`
verification. Configure `TROGON_SOURCE_TWITTER_CONSUMER_SECRET` before you
register the webhook URL with X.

## Notion webhooks

Notion signs webhook payloads with the subscription `verification_token`.
Configure `TROGON_SOURCE_NOTION_VERIFICATION_TOKEN` before starting the gateway,
then point the Notion webhook endpoint at `/notion/webhook`. Verified events
are forwarded to NATS on `{subject_prefix}.{type}` subjects such as
`notion.page.created`.

## Sentry webhooks

Sentry integration-platform webhooks sign the raw JSON body with the app client
secret. Configure `TROGON_SOURCE_SENTRY_CLIENT_SECRET`, point the webhook URL
at `/sentry/webhook`, and the gateway will forward verified payloads to NATS on
`{subject_prefix}.{resource}.{action}` subjects such as `sentry.issue.created`.

## Exposing webhooks with ngrok

```bash
docker compose --profile dev up
```

This starts ngrok alongside the gateway. Check `docker compose logs ngrok`
for the public URL. Append the source prefix path when configuring each
platform's webhook settings (e.g. `https://<ngrok-url>/github/webhook` or
`https://<ngrok-url>/twitter/webhook`).

## Verify

Subscribe to NATS to see events flowing:

```bash
nats sub -s nats://nats.trogonai.orb.local:4222 ">"
```

## Environment variables

See `devops/docker/compose/.env.example` for the full list of configurable
env vars per source. All env vars use the `TROGON_SOURCE_<SOURCE>_` prefix.

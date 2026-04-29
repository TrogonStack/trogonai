# trogon-gateway

How to run the unified ingress gateway that receives source events and publishes them to NATS JetStream.

## What it runs

`trogon-gateway` starts:

- One HTTP server for webhook-based sources
- Optional Discord Gateway (WebSocket) runner when Discord is enabled
- JetStream stream provisioning for every enabled source
- Health probes at `GET /-/liveness` and `GET /-/readiness`

If no source is configured, startup fails.

## Quick start

From `rsworkspace/`:

```bash
cargo run -p trogon-gateway -- serve
```

Use a config file if needed:

```bash
cargo run -p trogon-gateway -- serve --config ./gateway.toml
```

## Webhook routes

All webhook sources share one HTTP port (`TROGON_GATEWAY_PORT`, default `8080`) and are mounted by prefix:

| Source | Route |
|---|---|
| GitHub | `/github/webhook` |
| Slack | `/slack/webhook` |
| Telegram | `/telegram/webhook` |
| Twitter/X | `/twitter/webhook` |
| GitLab | `/gitlab/webhook` |
| Linear | `/linear/webhook` |
| Microsoft Teams | `/microsoft-teams/webhook` |
| Notion | `/notion/webhook` |
| Sentry | `/sentry/webhook` |

## Source enablement

A source is enabled only when its required setting is present:

| Source | Required setting |
|---|---|
| GitHub | `TROGON_SOURCE_GITHUB_WEBHOOK_SECRET` |
| Discord | `TROGON_SOURCE_DISCORD_BOT_TOKEN` |
| Slack | `TROGON_SOURCE_SLACK_SIGNING_SECRET` |
| Telegram | `TROGON_SOURCE_TELEGRAM_WEBHOOK_SECRET` |
| Twitter/X | `TROGON_SOURCE_TWITTER_CONSUMER_SECRET` |
| GitLab | `TROGON_SOURCE_GITLAB_WEBHOOK_SECRET` |
| Linear | `TROGON_SOURCE_LINEAR_WEBHOOK_SECRET` |
| Microsoft Teams | `TROGON_SOURCE_MICROSOFT_TEAMS_CLIENT_STATE` |
| Notion | `TROGON_SOURCE_NOTION_VERIFICATION_TOKEN` |
| Sentry | `TROGON_SOURCE_SENTRY_CLIENT_SECRET` |

## Core configuration

Gateway-level settings:

- `TROGON_GATEWAY_PORT` (default: `8080`)
- `NATS_URL` (default: `localhost:4222`, comma-separated supported)

NATS auth is resolved in this priority order:

1. `NATS_CREDS`
2. `NATS_NKEY`
3. `NATS_USER` + `NATS_PASSWORD`
4. `NATS_TOKEN`

Per-source optional tuning (with defaults):

- `TROGON_SOURCE_<SOURCE>_SUBJECT_PREFIX` (defaults include `github`, `discord`, `slack`, `telegram`, `twitter`, `gitlab`, `linear`, `microsoft-teams`, `incidentio`, `notion`, `sentry`)
- `TROGON_SOURCE_<SOURCE>_STREAM_NAME` (defaults include `GITHUB`, `DISCORD`, `SLACK`, `TELEGRAM`, `TWITTER`, `GITLAB`, `LINEAR`, `MICROSOFT_TEAMS`, `INCIDENTIO`, `NOTION`, `SENTRY`)
- `TROGON_SOURCE_<SOURCE>_STREAM_MAX_AGE_SECS` (default: `604800`)
- `TROGON_SOURCE_<SOURCE>_NATS_ACK_TIMEOUT_SECS` (default: `10`)

Source-specific extras:

- `TROGON_SOURCE_DISCORD_GATEWAY_INTENTS`
- `TROGON_SOURCE_MICROSOFT_TEAMS_CLIENT_STATE` must match the `clientState` used when creating Microsoft Graph subscriptions
- `TROGON_SOURCE_SLACK_TIMESTAMP_MAX_DRIFT_SECS` (default: `300`)
- `TROGON_SOURCE_LINEAR_TIMESTAMP_TOLERANCE_SECS` (default: `60`, `0` disables tolerance)
- `TROGON_SOURCE_TWITTER_CONSUMER_SECRET` is used for both CRC responses and `x-twitter-webhooks-signature` validation

## Config file shape

Environment variables can be replaced (or mixed) with a TOML config file:

```toml
[http_server]
port = 8080

[nats]
url = "localhost:4222"
# creds = "/path/to/nats.creds"
# nkey = "SU..."
# user = "my-user"
# password = "my-pass"
# token = "my-token"

[sources.github]
webhook_secret = "gh-secret"

[sources.discord]
bot_token = "<discord-bot-token>"

[sources.slack]
signing_secret = "slack-secret"

[sources.telegram]
webhook_secret = "telegram-secret"

[sources.twitter]
consumer_secret = "twitter-consumer-secret"

[sources.gitlab]
webhook_secret = "gitlab-secret"

[sources.linear]
webhook_secret = "linear-secret"

[sources.microsoft_teams]
client_state = "microsoft-teams-client-state"

[sources.notion]
verification_token = "notion-verification-token-example"

[sources.sentry]
client_secret = "sentry-client-secret"
```

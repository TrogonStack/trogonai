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

All webhook sources share one HTTP port (`TROGON_GATEWAY_PORT`, default `8080`). Every webhook source is
configured as a named integration and mounted at its integration path.

| Source | Route |
|---|---|
| GitHub | `/sources/github/{integration}/webhook` |
| Slack | `/sources/slack/{integration}/webhook` |
| Telegram | `/sources/telegram/{integration}/webhook` |
| Twitter/X | `/sources/twitter/{integration}/webhook` |
| GitLab | `/sources/gitlab/{integration}/webhook` |
| incident.io | `/sources/incidentio/{integration}/webhook` |
| Linear | `/sources/linear/{integration}/webhook` |
| Microsoft Graph change notifications | `/sources/microsoft-graph/{integration}/webhook` |
| Notion | `/sources/notion/{integration}/webhook` |
| Sentry | `/sources/sentry/{integration}/webhook` |

Integration IDs must contain only ASCII letters, numbers, `_`, or `-`. They cannot be empty, contain path
separators, or exceed 64 characters.

## Source enablement

Webhook sources require TOML entries under `[sources.<source>.integrations.<integration>.webhook]`. Source-level
webhook secrets and implicit source-specific environment variables are not supported.
Webhook secret fields accept either a literal string or an explicit environment reference such as
`webhook_secret = { env = "GITHUB_ACME_MAIN_WEBHOOK_SECRET" }`.
Discord is also configured in TOML under `[sources.discord]`; its `bot_token` field accepts the same literal
or explicit environment reference shape.

| Source | Required setting |
|---|---|
| GitHub | `webhook_secret` |
| Discord | `bot_token` |
| Slack | `signing_secret` |
| Telegram | `webhook_secret` |
| Twitter/X | `consumer_secret` |
| GitLab | `webhook_secret` |
| incident.io | `signing_secret` |
| Linear | `webhook_secret` |
| Microsoft Graph change notifications | `client_state` |
| Notion | `verification_token` |
| Sentry | `client_secret` |

## Core configuration

Gateway-level settings:

- `TROGON_GATEWAY_PORT` (default: `8080`)
- `NATS_URL` (default: `localhost:4222`, comma-separated supported)

NATS auth is resolved in this priority order:

1. `NATS_CREDS`
2. `NATS_NKEY`
3. `NATS_USER` + `NATS_PASSWORD`
4. `NATS_TOKEN`

Discord optional tuning is configured in TOML under `[sources.discord]`:

- `gateway_intents`
- `subject_prefix` (default: `discord`)
- `stream_name` (default: `DISCORD`)
- `stream_max_age_secs` (default: `604800`)
- `nats_ack_timeout_secs` (default: `10`)

Integration-level optional tuning is configured in TOML with `subject_prefix`, `stream_name`,
`stream_max_age_secs`, and `nats_ack_timeout_secs`.

Source-specific extras:

- Microsoft Graph `client_state` must match the `clientState` used when creating Microsoft Graph subscriptions
- Slack `timestamp_max_drift_secs` (default: `300`)
- Telegram `webhook_registration_mode = "startup"` attempts registration on startup and requires `bot_token` plus `public_webhook_url`
- Linear `timestamp_tolerance_secs` (default: `60`, `0` disables tolerance)
- Twitter/X `consumer_secret` is used for both CRC responses and `x-twitter-webhooks-signature` validation

## Config file shape

Source configuration lives in TOML. Secret fields can reference environment variables explicitly:

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

[sources.github.integrations.acme-main.webhook]
webhook_secret = { env = "GITHUB_ACME_MAIN_WEBHOOK_SECRET" }

[sources.github.integrations.other-org]
# subject_prefix defaults to "github-other-org"
# stream_name defaults to "GITHUB_OTHER-ORG"

[sources.github.integrations.other-org.webhook]
webhook_secret = "other-org-secret"

[sources.discord]
bot_token = { env = "DISCORD_BOT_TOKEN" }

[sources.slack.integrations.primary.webhook]
signing_secret = "slack-secret"

[sources.telegram.integrations.primary.webhook]
webhook_secret = "telegram-secret"
# webhook_registration_mode = "startup"
# bot_token = { env = "TELEGRAM_PRIMARY_BOT_TOKEN" }
# public_webhook_url = "https://example.com/sources/telegram/primary/webhook"

[sources.twitter.integrations.primary.webhook]
consumer_secret = "twitter-consumer-secret"

[sources.gitlab.integrations.primary.webhook]
webhook_secret = "gitlab-secret"

[sources.incidentio.integrations.primary.webhook]
signing_secret = "whsec_dGVzdC1zZWNyZXQ="

[sources.linear.integrations.primary.webhook]
webhook_secret = "linear-secret"

[sources.microsoft_graph.integrations.primary.webhook]
client_state = "microsoft-graph-client-state"

[sources.notion.integrations.primary.webhook]
verification_token = "notion-verification-token-example"

[sources.sentry.integrations.primary.webhook]
client_secret = "sentry-client-secret"
```

## Microsoft Graph change notifications

This source receives Microsoft Graph change notifications. It does not implement
Bot Framework conversations or send replies.

Create Microsoft Graph subscriptions, then use `/sources/microsoft-graph/{integration}/webhook` as the
subscription `notificationUrl`.
The gateway responds to Graph's
`validationToken` challenge, validates each notification's `clientState`, and
publishes the Graph `changeNotificationCollection` to NATS on
`{subject_prefix}.change_notification_collection`, for example
`microsoft-graph.change_notification_collection`.

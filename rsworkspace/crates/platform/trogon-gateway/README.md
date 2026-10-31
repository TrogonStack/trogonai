# trogon-gateway

How to run the unified ingress gateway that receives source events and publishes them to NATS JetStream.

## What it runs

`trogon-gateway` starts:

- One HTTP server for webhook-based sources
- Optional Discord Gateway (WebSocket) runner when Discord is enabled
- Optional Google Cloud Pub/Sub StreamingPull runner per `google_pubsub` integration
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

## Container image

See [Container Images](../../../../docs/reference/container-images.md).

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
| Datadog | `/sources/datadog/{integration}/webhook` |
| Zoom | `/sources/zoom/{integration}/webhook` |
| Plaid | `/sources/plaid/{integration}/webhook` |

Integration IDs must contain only ASCII letters, numbers, `_`, or `-`. They cannot be empty, contain path
separators, or exceed 64 characters.

## Source enablement

Webhook sources require TOML entries under `[sources.<source>.integrations.<integration>.webhook]`. Source-level
webhook secrets and implicit source-specific environment variables are not supported.
Webhook credential fields accept either a literal string or an explicit environment reference such as
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
| GitLab | `signing_token` |
| incident.io | `signing_secret` |
| Linear | `webhook_secret` |
| Microsoft Graph change notifications | `client_state` |
| Notion | `verification_token` |
| Sentry | `client_secret` |
| Datadog | `webhook_token` |
| Zoom | `secret_token` |
| Plaid | `client_id`, `secret`, `environment` |

## Core configuration

Gateway-level settings:

- `TROGON_GATEWAY_PORT` (default: `8080`)
- `NATS_URL` (default: `localhost:4222`, comma-separated supported)

NATS auth accepts the same values as the `nats` CLI. One method is chosen in this priority order:

1. `NATS_CREDS` (path or `file://path`)
2. `NATS_NKEY` (an NKey seed, a seed file as a path or `file://path`, or `env://NAME`)
3. `NATS_USER` + `NATS_PASSWORD`
4. `NATS_TOKEN`, or `NATS_USER` set without `NATS_PASSWORD`
5. `NATS_JWT` + `NATS_SEED`, or `NATS_JWT` alone with an empty signature

`NATS_JWT`, `NATS_SEED`, `NATS_PASSWORD` and `NATS_TOKEN` accept `file://path` and `env://NAME` references. The `op`, `nsc` and `data` schemes are rejected at connect time.

TLS is configured with `NATS_CERT` + `NATS_KEY` (setting only one fails the connection) and `NATS_CA`. Setting any of them requires TLS.

The same settings can be given as flags (`--nats-user`, `--nats-password`, `--nats-creds`, `--nats-nkey`, `--nats-jwt`, `--nats-seed`, `--nats-token`, `--nats-tls-cert`, `--nats-tls-key`, `--nats-tls-ca`) or in the `[nats]` TOML section (`tls_cert`, `tls_key`, `tls_ca` for the TLS values).

Every process on the host can read a command line, so `--nats-nkey`, `--nats-password`, `--nats-token`, `--nats-jwt` and `--nats-seed` take only a reference, `env://NAME` or `file:///path`, and reject the secret itself. For the same reason `--nats-user` is rejected when no password or token is configured, because the user would then be sent as a token.

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
- Slack acknowledges within Slack's 3 second deadline, so the publish budget is 2.8 seconds, or 1 second when the delivery also waits for a consumer reply. `nats_ack_timeout_secs` defaults to that budget for Slack and a larger value is lowered to it with a warning at startup
- Slack webhook requests with `Content-Type: application/x-www-form-urlencoded` and exactly one `ssl_check=1` field are answered `200` before signature verification and are never published, matching Slack's connectivity probe
- Slack webhook `max_body_bytes` (default: `4194304`) is either an integer or `{ env = "VAR" }` naming an environment variable that holds the byte count. Requests above it are refused with `413`. It does not apply to Socket Mode
- Slack Socket Mode `intended_use` is required and must be `development` or `behind_firewall`, the two uses Slack names for Socket Mode. Slack does not intend Socket Mode for widely distributed apps, so a missing or different value is a configuration error. HTTP webhooks are the production transport
- Slack Socket Mode `api_base_url` (default: `https://slack.com/api/`) is the Slack Web API base the gateway calls `apps.connections.open` on. Set it to `https://slack-gov.com/api/` for GovSlack. It must be an absolute `http` or `https` URL
- Telegram `webhook_registration_mode = "startup"` attempts registration on startup and requires `bot_token` plus `public_webhook_url`
- Linear `timestamp_tolerance_secs` (default: `60`, `0` disables tolerance)
- Twitter/X `consumer_secret` is used for both CRC responses and `x-twitter-webhooks-signature` validation
- Datadog webhooks are not signed. `webhook_token` is a shared secret the operator sends as a custom header (configured in the Datadog webhook's *Custom Headers*), verified in constant time. The header name defaults to `X-Datadog-Webhook-Token` and can be overridden per integration with `webhook_token_header`. Routing keys off an `event_type` field in the payload template, falling back to `.unroutable` when it is absent or invalid; an optional `id` field is used as the NATS message ID for deduplication
- Datadog `timestamp_tolerance_secs` (optional, disabled by default) enables a freshness/anti-replay check. Datadog sends no timestamp header, so the operator must template a `timestamp` field carrying `$DATE_POSIX` (POSIX seconds) into the payload. When enabled, requests whose `timestamp` is outside the window are rejected (`401`), and requests missing/with an unparseable `timestamp` are rejected (`400`). `0` or unset disables the check
- Zoom `secret_token` is the app's Secret Token. Requests are verified with `x-zm-signature` over `v0:{x-zm-request-timestamp}:{body}`, and `timestamp_tolerance_secs` (default: `300`) bounds how far `x-zm-request-timestamp` may drift from the gateway clock. Zoom provides no stable per-delivery identifier, so no NATS message ID is set and retries are not deduplicated
- Plaid `client_id` is either a string or `{ env = "VAR" }` naming an environment variable that holds it; it is an identifier, not a secret. Plaid `secret` is the API secret for the `client_id` and `environment` (`sandbox` or `production`) that own the webhooks. The gateway verifies the `Plaid-Verification` JWT (ES256 only) against the key Plaid serves for its `kid` from `/webhook_verification_key/get`, caches keys by `kid`, rejects expired keys, and requires `iat` within `timestamp_tolerance_secs` (default: `300`, `0` is rejected) of the gateway clock and `request_body_sha256` to match the SHA-256 of the raw body. `api_base_url` optionally overrides the host derived from `environment`

## Config file shape

Source configuration lives in TOML. Secret fields can reference environment variables explicitly. A secret value marked as `secret:<nid>:<nss>:<value>` is used without its marker, and a value starting with `base64:` is decoded once at startup, so multi-line or JSON secrets can be stored as a single line such as `secret:gcp:service-account:base64:...`:

```toml
[http_server]
port = 8080

[nats]
url = "localhost:4222"
# creds = "/path/to/nats.creds"
# nkey = "SU..." # or "/path/to/user.nk"
# user = "my-user"
# password = "my-pass"
# token = "my-token"
# jwt = "file:///path/to/user.jwt"
# seed = "env://NATS_USER_SEED"
# tls_cert = "/path/to/client.crt"
# tls_key = "/path/to/client.key"
# tls_ca = "/path/to/ca.pem"

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
signing_token = "whsec_<base64-encoded-32-byte-secret>"
timestamp_tolerance_secs = 300

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

[sources.datadog.integrations.primary.webhook]
webhook_token = { env = "DATADOG_PRIMARY_WEBHOOK_TOKEN" }
# webhook_token_header = "X-Datadog-Webhook-Token"  # optional; override the custom header name
# timestamp_tolerance_secs = 300  # optional anti-replay window; requires "timestamp": "$DATE_POSIX" in the payload

[sources.zoom.integrations.primary.webhook]
secret_token = { env = "ZOOM_PRIMARY_SECRET_TOKEN" }
# timestamp_tolerance_secs = 300  # optional; max drift of x-zm-request-timestamp

[sources.plaid.integrations.primary.webhook]
client_id = "your-plaid-client-id"  # or { env = "PLAID_PRIMARY_CLIENT_ID" }
secret = { env = "PLAID_PRIMARY_SECRET" }
environment = "production"  # sandbox or production
# timestamp_tolerance_secs = 300  # optional; max drift of the JWT iat claim
```

Configure the matching Datadog webhook (Integrations > Webhooks) with a custom header and a payload template, for example:

```json
{
  "X-Datadog-Webhook-Token": "$WEBHOOK_TOKEN"
}
```

```json
{
  "id": "$ID",
  "event_type": "$EVENT_TYPE",
  "timestamp": "$DATE_POSIX",
  "title": "$EVENT_TITLE",
  "body": "$EVENT_MSG",
  "alert_transition": "$ALERT_TRANSITION"
}
```

`timestamp` is only required when `timestamp_tolerance_secs` is set; `$DATE_POSIX` is POSIX epoch seconds (`$DATE` is epoch milliseconds, which this source does not accept).

## Slack

HTTP webhooks are the production transport. Socket Mode is for internal apps that are in development or deployed
behind a firewall, and Slack does not intend it for widely distributed apps. An app that uses Socket Mode cannot be
listed in the Slack Marketplace ([Slack: using Socket Mode](https://docs.slack.dev/apis/events-api/using-socket-mode)). Each Socket Mode integration must set
`intended_use` under `socket_mode` to `development` or `behind_firewall`, and the gateway logs it when the runner
starts.

```toml
[sources.slack.integrations.primary.socket_mode]
app_token = { env = "SLACK_PRIMARY_APP_TOKEN" }
intended_use = "behind_firewall"
```

Slack retries an Events API delivery that was not acknowledged, once almost immediately, again after about a
minute and again after about five minutes. The gateway keeps the stream `duplicate_window` at ten minutes, or at
the stream `max_age` when that is shorter, and deduplicates on `Nats-Msg-Id`, so those retries are stored once. The
window is reconciled on every start, so a stream created before this setting existed picks it up on restart.

- Deduplication covers Slack's standard retry schedule only. A redelivery that arrives after the window, such as a
  late retry or a Delayed Events style replay, is stored again, and consumers must tolerate duplicates.
- Payloads are stored exactly as Slack sent them, including the verification `token` field Slack still includes in
  Events API bodies. The stream `max_age` is therefore also how long those tokens stay readable in NATS, so size
  it and restrict access to the stream accordingly.
- A modal `view_submission` and a slash command can be answered inside Slack's acknowledgement. After the publish is
  stored, the gateway sends a core NATS request to `{subject_prefix}.respond.interaction.view_submission` or
  `{subject_prefix}.respond.command.{name}` with the published body and headers, plus `X-Slack-Callback-Id` for
  modal submissions, and forwards the reply. Run a responder on those subjects, for example a `response_action`
  of `errors` for a modal or an in channel message for a command, and set `Content-Type` on the reply.
- Over HTTP the reply body and its `Content-Type` become the response. Over Socket Mode the reply becomes the ack
  `payload`, and a `text/plain` reply is sent as `{"text": ...}`. An empty reply is a plain acknowledgement.
- When no responder is subscribed, or the reply is later than Slack's deadline minus a 200 millisecond margin, the
  gateway answers HTTP `404` and sends no Socket Mode ack. The delivery is still stored, so a command that must
  always acknowledge needs a responder that is always running.
- An external select asks for its options with a `block_suggestion`. It is never published or stored: the gateway
  sends only a core NATS request to `{subject_prefix}.respond.interaction.block_suggestion` with the payload and
  headers plus `X-Slack-Action-Id` and `X-Slack-Block-Id`, and the reply carrying `options` or `option_groups` is
  the response. The same `404` and no ack rules apply when nothing answers.
- The respond subjects are outside the stream filters on purpose, so replies are never stored.

## Zoom

Subscribe a Zoom app's Event Subscriptions to `https://<gateway-host>/sources/zoom/{integration}/webhook` and
set the app's Secret Token as `secret_token`. The gateway verifies the signature and timestamp on every request,
answers the `endpoint.url_validation` handshake inline with the `plainToken` and its `encryptedToken` (no NATS
publish), and publishes every other event body unchanged to `{subject_prefix}.{event}`. Dots in Zoom event
names become subject tokens, so `recording.transcript_completed` lands on
`zoom-primary.recording.transcript_completed` and `meeting.ended` on `zoom-primary.meeting.ended`. Bodies that
are not JSON, lack `event`, or carry an event name that is not a valid subject go to `{subject_prefix}.unroutable`
with an `X-Zoom-Reject-Reason` header. This source does not call the Zoom API; consumers fetch recordings or
transcripts themselves.

## Plaid

Set the webhook URL of a Plaid Item or Link token to `https://<gateway-host>/sources/plaid/{integration}/webhook`.
Every request carries a `Plaid-Verification` JWT. The gateway requires `alg` to be `ES256`, fetches the
verification key for the token's `kid` with the configured `client_id` and `secret`, verifies the signature,
checks `iat` against `timestamp_tolerance_secs`, and compares `request_body_sha256` with the SHA-256 of the raw
body in constant time. Keys are cached per `kid` and fetched again after an hour, so a key that Plaid marks
expired stops verifying within the hour. A missing or invalid token, a stale `iat`, a body hash mismatch, or an
expired key returns `401`, and so does a `kid` that Plaid reports as unknown, so a made-up `kid` cannot pose as
an outage. Plaid also reports a genuine `kid` as unknown when it belongs to a different Plaid environment, and that
`401` is a permanent failure, so if every webhook is rejected with an unknown `kid` warning, check that
`environment` matches the environment the webhooks come from. Any other failure to fetch the key, including the hourly fetch of a cached key, returns
`503`, so a misconfigured `secret` makes Plaid retry for up to 24 hours instead of dropping the webhook as a
permanent `4xx` failure. Fetches for a `kid` that is not cached are limited to one every 10 seconds per
integration, and a cached `kid` whose hourly fetch failed is not fetched again for 10 seconds. Requests over
either limit get `429` with `Retry-After: 10`, which Plaid waits out instead of backing off exponentially, so the
webhook is retried once a fetch is allowed again. With `TROGON_GATEWAY_ENABLE_RUNTIME_PLAID_CREDENTIALS` enabled,
the gateway resolves the `secret` from the credential store on every request instead of using the configured
value, so revoking the credential stops ingestion even though verification keys stay cached.

Verified bodies are published unchanged to `{subject_prefix}.{webhook_type}.{webhook_code}`, for example
`plaid-primary.TRANSACTIONS.SYNC_UPDATES_AVAILABLE`, with `X-Plaid-Event-Type` and, when the payload carries an
`item_id`, `X-Plaid-Item-Id`. `Nats-Msg-Id` is the SHA-256 of the `Plaid-Verification` token, which Plaid signs for
every delivery attempt, and the stream `duplicate_window` is twice `timestamp_tolerance_secs`, or the stream
`max_age` when that is shorter. Replays of one signed request are dropped, while retries and distinct webhooks with
identical bodies are all published, so consumers should treat Plaid webhooks as at-least-once. Bodies that are not
JSON, lack `webhook_type` or `webhook_code`, or produce an invalid subject go to `{subject_prefix}.unroutable` with
an `X-Plaid-Reject-Reason` header.

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

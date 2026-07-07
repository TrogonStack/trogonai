# Receiving Source Events Locally

`trogon-gateway` is the unified inbound pipe for platform events into NATS
JetStream. It runs all configured sources in a single process behind a single
HTTP server, plus the optional Discord gateway runner.

All webhook sources share one port (default `8080`). Webhooks are configured as
named TOML integrations, each with its own literal or env-backed secret.

| Source | Webhook path | Required webhook field |
|---|---|---|
| GitHub | `/sources/github/{integration}/webhook` | `webhook_secret` |
| Slack | `/sources/slack/{integration}/webhook` | `signing_secret` |
| Telegram | `/sources/telegram/{integration}/webhook` | `webhook_secret` |
| Twitter/X | `/sources/twitter/{integration}/webhook` | `consumer_secret` |
| GitLab | `/sources/gitlab/{integration}/webhook` | `signing_token` |
| incident.io | `/sources/incidentio/{integration}/webhook` | `signing_secret` |
| Linear | `/sources/linear/{integration}/webhook` | `webhook_secret` |
| Microsoft Graph change notifications | `/sources/microsoft-graph/{integration}/webhook` | `client_state` |
| Notion | `/sources/notion/{integration}/webhook` | `verification_token` |
| Sentry | `/sources/sentry/{integration}/webhook` | `client_secret` |

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
# edit .env for gateway, NATS, source secrets, logging, or ngrok settings
docker compose up --build --remove-orphans
```

The compose stack mounts `./services/trogon-gateway/gateway.toml` into the
gateway container. The default file enables a local GitHub receiver at
`/sources/github/local/webhook` using `TROGON_GATEWAY_LOCAL_GITHUB_WEBHOOK_SECRET`
from `.env`.
That variable is only for this local compose config; other environments only
need the environment variables referenced by their deployed gateway TOML.

## Webhook integrations

Edit or replace `devops/docker/compose/services/trogon-gateway/gateway.toml` from the repository root to configure webhook
sources:

```toml
[sources.github.integrations.acme-main.webhook]
webhook_secret = { env = "GITHUB_ACME_MAIN_WEBHOOK_SECRET" }

[sources.github.integrations.acme-eu.webhook]
webhook_secret = { env = "GITHUB_ACME_EU_WEBHOOK_SECRET" }
```

These integrations are served at `/sources/github/acme-main/webhook` and
`/sources/github/acme-eu/webhook`. Their default NATS subjects are
`github-acme-main.>` and `github-acme-eu.>`, with streams `GITHUB_ACME-MAIN`
and `GITHUB_ACME_EU`.

Values referenced with `{ env = "..." }` are read from the gateway process
environment. With Docker Compose, put those variables in `.env`; the
`trogon-gateway` service loads that file.

## Discord gateway

Configure `[sources.discord]` in `devops/docker/compose/services/trogon-gateway/gateway.toml` from the repository root. For
local secrets, point `bot_token` at an explicit env var and put that variable in
`.env`:

```toml
[sources.discord]
bot_token = { env = "DISCORD_BOT_TOKEN" }
```

Discord does not use the HTTP ingress or ngrok. It opens an outbound WebSocket
connection to Discord and publishes every gateway event to NATS.

## Slack Socket Mode

Slack integrations can use either the existing HTTP webhook transport or Socket
Mode. Configure exactly one transport per integration. Socket Mode does not use
the HTTP ingress or ngrok; it opens an outbound WebSocket to Slack and publishes
Events API payloads, interactive payloads, and slash commands to the same NATS
subjects as the webhook transport.

```toml
[sources.slack.integrations.primary]
subject_prefix = "slack-primary"
stream_name = "SLACK_PRIMARY"

[sources.slack.integrations.primary.socket_mode]
app_token = { env = "SLACK_PRIMARY_APP_TOKEN" }
```

## Telegram webhooks

Set `webhook_secret` under `[sources.telegram.integrations.<integration>.webhook]`. To let
the gateway register the webhook on startup, also set
`webhook_registration_mode = "startup"`, `bot_token`, and
`public_webhook_url` in that integration's webhook block. The URL must be the public HTTPS endpoint
for `/sources/telegram/{integration}/webhook`.

In `manual` mode, the gateway only serves the local webhook receiver and the
Telegram webhook must be configured separately. In `startup` mode, the bot token
and public webhook URL are required. Registration failures are logged and do not
block the gateway from serving configured sources.

## incident.io webhooks

incident.io uses Svix-style webhook signing. Set webhook `signing_secret` for the
integration to the `whsec_...` signing secret from incident.io and configure the webhook
endpoint as `/sources/incidentio/{integration}/webhook`.

incident.io does not guarantee ordered delivery, and private incident events may
contain only resource IDs rather than full objects. The gateway forwards the raw
verified payload to NATS and leaves any enrichment or reordering to downstream
consumers.

## Twitter/X webhooks

X uses the app consumer secret for both the CRC challenge response and
`x-twitter-webhooks-signature` verification. Configure webhook
`consumer_secret` before you register `/sources/twitter/{integration}/webhook` with X.

## Notion webhooks

Notion signs webhook payloads with the subscription `verification_token`.
Configure webhook `verification_token` for the integration before starting the gateway, then point
the Notion webhook endpoint at `/sources/notion/{integration}/webhook`. Verified events
are forwarded to NATS on `{subject_prefix}.{type}` subjects.

## Sentry webhooks

Sentry integration-platform webhooks sign the raw JSON body with the app client
secret. Configure webhook `client_secret` for the integration, point the webhook URL at
`/sources/sentry/{integration}/webhook`, and the gateway will forward verified payloads to
NATS on `{subject_prefix}.{resource}.{action}` subjects.

## Microsoft Graph change notifications

This source receives Microsoft Graph change notifications. It does not implement
Bot Framework conversations or send replies.

Configure webhook `client_state` with the same secret `clientState` used when
creating the Graph subscription, then point the subscription `notificationUrl`
at `/sources/microsoft-graph/{integration}/webhook`. The gateway answers
Graph's `validationToken` handshake and forwards each validated Graph
notification collection to NATS on
`{subject_prefix}.change_notification_collection`, for example
`microsoft-graph.change_notification_collection`. The collection publish uses a
deterministic NATS message ID derived from the Graph notification identities so
exact webhook retries can be deduplicated at the collection boundary.

For subscriptions that include resource data, the gateway preserves Graph's
collection payload, including `validationTokens`. Downstream consumers remain
responsible for validating those tokens, decrypting `encryptedContent`, and
splitting individual notifications when they need per-resource routing.

## Exposing webhooks with ngrok

```bash
docker compose --profile dev up
```

This starts ngrok alongside the gateway. Check `docker compose logs ngrok`
for the public URL. Append the integration path when configuring each platform's
webhook settings (for example, `https://<ngrok-url>/sources/github/acme-main/webhook`).

## OpenBao local secret store

The compose stack starts OpenBao in dev mode at:

```bash
http://openbao.trogonai.orb.local:8200
```

The local dev token is:

```bash
dev-only-token
```

This OpenBao instance is in-memory and local-only. Do not use it for production
or for any secret that matters outside the dev stack.

### Copy-in and copy-out smoke test

From `devops/docker/compose`:

```bash
docker compose up -d openbao
docker compose exec openbao bao kv put -mount=secret trogonai/manual/copy-in-out value=copy-this-value-in-and-out
docker compose exec openbao bao kv get -format=json -mount=secret trogonai/manual/copy-in-out
```

The JSON output should include:

```json
{
  "data": {
    "data": {
      "value": "copy-this-value-in-and-out"
    }
  }
}
```

Automated Rust tests start their own OpenBao container with Testcontainers:

```bash
cd ../../../rsworkspace
mise exec -- cargo test -p trogon-gateway openbao_testcontainer_roundtrips_precise_value
mise exec -- cargo test -p trogon-gateway runtime_handler_with_openbao_testcontainer_applies_lifecycle_and_resolves_precise_value
```

To run the same gateway adapter round trip against this compose service:

```bash
cd ../../../rsworkspace
OPENBAO_ADDR=http://openbao.trogonai.orb.local:8200 OPENBAO_TOKEN=dev-only-token mise exec -- cargo test -p trogon-gateway openbao_dev_server_roundtrips_precise_value -- --ignored --nocapture
```

The adapter tests write `copy-this-value-in-and-out` into OpenBao and read it
back through `OpenBaoSecretStore`. The lifecycle-runtime test writes that same
value through `CredentialLifecycleRuntimeHandler`, resolves it through the
runtime registry, rotates it to `rotated-copy-this-value-in-and-out`, revokes
it, and verifies lifecycle events do not contain plaintext. The compose-backed
ignored test also prints the credential ref plus the API data and metadata
paths.

### Management API smoke test

Enable the internal management API and one runtime source in `.env`:

```bash
TROGON_GATEWAY_CREDENTIAL_MANAGEMENT_ADMIN_TOKEN=local-admin-token
TROGON_GATEWAY_ENABLE_RUNTIME_GITHUB_CREDENTIALS=true
```

Then start the stack:

```bash
docker compose up --build --remove-orphans
```

Create a GitHub webhook secret:

```bash
curl -sS -X PUT "http://trogon-gateway.trogonai.orb.local:8080/-/credentials/github/primary/webhook-secret" \
  -H "content-type: application/json" \
  -H "x-trogon-admin-token: local-admin-token" \
  -H "Idempotency-Key: local-github-primary-webhook-secret-create-1" \
  -d '{"owner_id":"tenant-1","secret":"copy-this-value-in-and-out"}'
```

Rotate it:

```bash
curl -sS -X POST "http://trogon-gateway.trogonai.orb.local:8080/-/credentials/github/primary/webhook-secret/rotations" \
  -H "content-type: application/json" \
  -H "x-trogon-admin-token: local-admin-token" \
  -H "Idempotency-Key: local-github-primary-webhook-secret-rotate-1" \
  -d '{"owner_id":"tenant-1","version":1,"secret":"rotated-copy-this-value-in-and-out"}'
```

Revoke the rotated version:

```bash
curl -sS -X DELETE "http://trogon-gateway.trogonai.orb.local:8080/-/credentials/github/primary/webhook-secret" \
  -H "content-type: application/json" \
  -H "x-trogon-admin-token: local-admin-token" \
  -H "Idempotency-Key: local-github-primary-webhook-secret-revoke-1" \
  -d '{"owner_id":"tenant-1","version":2}'
```

Check recovery worker status:

```bash
curl -sS "http://trogon-gateway.trogonai.orb.local:8080/-/credentials/recovery/status" \
  -H "x-trogon-admin-token: local-admin-token"
```

The create, rotate, and revoke responses return credential refs, lifecycle
state, and stream position. They must not echo the submitted secret value.

To inspect an API-created value directly in local OpenBao, use the logical
credential id from the response as the final path segment:

```bash
docker compose exec openbao bao kv get -format=json -mount=secret \
  "trogonai/tenant-1/credentials/openbao:tenant-1:github/primary:webhook_secret"
```

Do not percent-encode this value for the `bao` CLI. The HTTP adapter URL-encodes
path segments internally when it calls OpenBao.

## Verify

Subscribe to NATS to see events flowing:

```bash
nats sub -s nats://nats.trogonai.orb.local:4222 ">"
```

## Environment variables

See `devops/docker/compose/.env.example` for gateway, NATS, logging, local
tunnel env vars, and secret values referenced explicitly from `gateway.toml`.
Source integrations are configured in TOML.

# Receiving Source Events Locally

`trogon-gateway` is the unified inbound pipe for platform events into NATS
JetStream. It runs all configured sources in a single process behind a single
HTTP server, plus the optional outbound runners (Discord gateway, Slack Socket
Mode, Google Cloud Pub/Sub StreamingPull).

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
| Datadog | `/sources/datadog/{integration}/webhook` | `webhook_token` |
| Zoom | `/sources/zoom/{integration}/webhook` | `secret_token` |

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
printf 'TROGON_GATEWAY_LOCAL_GITHUB_WEBHOOK_SECRET=%s\n' "$(openssl rand -hex 32)" >> .env
# edit .env for gateway, source secrets, logging, or ngrok settings
docker compose up --build --remove-orphans
```

The compose stack mounts `./services/trogon-gateway/gateway.toml` into the
gateway container. The default file enables a local GitHub receiver at
`/sources/github/local/webhook` using `TROGON_GATEWAY_LOCAL_GITHUB_WEBHOOK_SECRET`
from `.env`. It has no default, and every `docker compose` command for this
stack, `down` included, fails until it is set.
That variable is only for this local compose config; other environments only
need the environment variables referenced by their deployed gateway TOML.

NATS runs the way [a deployment must](../../../../nats/README.md): TLS with a
verified client certificate, operator-mode accounts, and an encrypted
JetStream store. The first `up` generates the dev CA, the operator, its
accounts and every client's credentials into the `dev_secrets` volume, and the
gateway connects to the `PLATFORM` account. `docker compose down -v` discards
all of it together with the JetStream store, which nothing else can decrypt.

A `nats_data` volume from before this layout still holds the old unencrypted
store. Discard it once:

```bash
docker compose down
docker volume rm trogonai_nats_data
```

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

Secret values follow
[Base64-encoded configuration values](https://github.com/straw-hat-team/adr/blob/main/src/adrs/4689777057/README.md)
and
[Secret Value Syntax](https://github.com/straw-hat-team/adr/blob/main/src/adrs/3610135900/README.md).
A value marked as `secret:<nid>:<nss>:<value>` is used without its marker,
and a value that starts with `base64:` is decoded once at startup. Compose
does not run commands inside `.env`, so write the finished single-line value
into the file:

```sh
printf 'GITHUB_ACME_EU_WEBHOOK_SECRET=secret:github:webhook:base64:%s\n' \
  "$(printf '%s' "$WEBHOOK_SECRET" | base64 | tr -d '\n')" >> .env
```

Encode only values the carrier cannot hold unchanged, such as multi-line JSON
or PEM in a `.env` file. Single-line values like API tokens or `whsec_`
secrets, and values delivered by a Kubernetes Secret, are used as they are.
Keep the `secret:` marker outside the encoding so scanners still see it. Line
breaks inside the base64 are ignored. A value that is not valid base64, or
does not decode to UTF-8, fails startup with an error naming the integration
and field but never the value.

When upgrading, a secret whose literal value already starts with `base64:` is
now decoded. To keep that value unchanged, encode the whole value, prefix
included, and store the result as `base64:<encoded>`.

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

HTTP webhooks are the production transport for Slack integrations. Socket Mode
is the alternative for internal apps that are in development or deployed behind
a firewall. Slack does not intend it for widely distributed apps, and an app
that uses it cannot be listed in the Slack Marketplace
([Slack: using Socket Mode](https://docs.slack.dev/apis/events-api/using-socket-mode)).

Configure exactly one transport per integration. Socket Mode does not use the
HTTP ingress or ngrok; it opens an outbound WebSocket to Slack and publishes
Events API payloads, interactive payloads, and slash commands to the same NATS
subjects as the webhook transport.

`intended_use` is required and states why this integration uses Socket Mode.
The accepted values are `development` and `behind_firewall`. A missing or
different value is a configuration error.

```toml
[sources.slack.integrations.primary]
subject_prefix = "slack-primary"
stream_name = "SLACK_PRIMARY"

[sources.slack.integrations.primary.socket_mode]
app_token = { env = "SLACK_PRIMARY_APP_TOKEN" }
intended_use = "development"
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

## Google Cloud Pub/Sub

This source pulls messages from an existing Pub/Sub subscription and publishes
each one to NATS JetStream. It is generic: the gateway does not look inside the
message body, so any publisher that writes to the topic behind the
subscription can feed it.

```toml
[sources.google_pubsub.integrations.primary]
subject_prefix = "google-pubsub-primary"
stream_name = "GOOGLE_PUBSUB_PRIMARY"

[sources.google_pubsub.integrations.primary.streaming_pull]
subscription = "projects/my-project/subscriptions/my-subscription"
credentials = { env = "GOOGLE_PUBSUB_PRIMARY_CREDENTIALS" }
```

StreamingPull is an outbound gRPC connection from the gateway to Google, so it
does not use the HTTP ingress, a public URL, or ngrok. The subscription must
already exist. The optional `max_outstanding_messages` bounds how many
unacknowledged messages the client holds at once.

Every integration carries its own `credentials`: a Google credentials JSON
document, given either as a literal string or as `{ env = "..." }`. Prefer the
`env` form so the document stays out of `gateway.toml`, and use a
`secret:gcp:service-account:base64:...` value when the JSON must fit on one
line. The gateway never falls
back to any credentials it did not receive through this field, so integrations
for different projects or accounts can run side by side in one gateway. Two
document types are accepted, selected by the `type` field inside the JSON:

- `service_account`: a service account key.
- `external_account`: a workload identity federation configuration, which
  exchanges a token from your identity provider for Google access and needs no
  long-lived key.

Any other type, including `authorized_user`, is rejected when the configuration
loads, and so is a document that is not a JSON object. A document whose fields
are missing or malformed stops the gateway at startup with an error naming the
integration. Error messages never include the document. The identity needs the `roles/pubsub.subscriber` role on the
subscription.

Each message is published to `{subject_prefix}.message`. When the Pub/Sub
`data` fits within the NATS server's payload limit, it is the body unchanged.
Larger data is stored in the gateway's claim-check object store instead, and
the message carries an empty body with `Trogon-Claim-Check`,
`Trogon-Claim-Bucket`, and `Trogon-Claim-Key` headers. Consumers must resolve
those headers against the object store, for example with `ClaimResolver` from
`trogon-nats`, to recover the original data. The Pub/Sub message ID is used as
the `Nats-Msg-Id`, so redeliveries are deduplicated inside the stream's
duplicate window. These headers accompany every message:

| Header | Value |
|---|---|
| `X-PubSub-Message-Id` | Pub/Sub message ID |
| `X-PubSub-Publish-Time` | Publish time as an RFC 3339 timestamp |
| `X-PubSub-Ordering-Key` | Ordering key, only when the message has one |
| `X-PubSub-Subscription` | Full subscription name |
| `X-PubSub-Attribute-{key}` | One header per message attribute |

Attributes whose key is not a valid header name, or whose value contains a line
break, are skipped with a warning.

The gateway acknowledges a message to Pub/Sub only after JetStream has
confirmed the publish. If storing a claim-check payload or the publish fails or
times out, the message is
negatively acknowledged and Pub/Sub redelivers it. When the stream breaks, the
gateway logs the error, backs off, and subscribes again. On shutdown it stops
pulling and returns messages it has not handled to Pub/Sub.

## Exposing webhooks with ngrok

The `dev` profile publishes the gateway on a public ngrok URL. Anyone who has
the URL reaches the whole gateway, including the `/-/credentials` API when
`TROGON_GATEWAY_CREDENTIAL_MANAGEMENT_ADMIN_TOKEN` is set, so run it only while
a provider needs to call in.

```bash
docker compose --profile dev up
```

This starts ngrok alongside the gateway, and ngrok logs a warning that the
stack is public. Check `docker compose logs ngrok`
for the public URL. Append the integration path when configuring each platform's
webhook settings (for example, `https://<ngrok-url>/sources/github/acme-main/webhook`).
Close the tunnel as soon as you are done:

```bash
docker compose --profile dev stop ngrok
```

## OpenBao local secret store

The compose stack runs a local OpenBao at:

```bash
http://openbao.trogonai.orb.local:8200
```

The local dev token is:

```bash
local-dev-root-token
```

This OpenBao instance is local-only, auto-unsealed from a checked-in static
seal key. Do not use it for production or for any secret that matters outside
the dev stack.

### Copy-in and copy-out smoke test

From `devops/docker/compose`:

```bash
docker compose up -d openbao
docker compose exec -e BAO_ADDR=http://127.0.0.1:8200 -e BAO_TOKEN=local-dev-root-token openbao \
  bao kv put -mount=secret trogonai/manual/copy-in-out value=copy-this-value-in-and-out
docker compose exec -e BAO_ADDR=http://127.0.0.1:8200 -e BAO_TOKEN=local-dev-root-token openbao \
  bao kv get -format=json -mount=secret trogonai/manual/copy-in-out
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
mise exec -- cargo test -p trogon-gateway runtime_handler_with_openbao_testcontainer_applies_state_and_resolves_precise_value
```

To run the same gateway adapter round trip against this compose service:

```bash
cd ../../../rsworkspace
OPENBAO_ADDR=http://openbao.trogonai.orb.local:8200 OPENBAO_TOKEN=local-dev-root-token mise exec -- cargo test -p trogon-gateway openbao_dev_server_roundtrips_precise_value -- --ignored --nocapture
```

The adapter tests write `copy-this-value-in-and-out` into OpenBao and read it
back through `OpenBaoSecretStore`. The lifecycle-runtime test writes that same
value through `CredentialRuntimeHandler`, resolves it through the
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

The flag switches every configured GitHub integration to runtime credential
resolution, so the shipped `gateway.toml` integration `local` stops reading its
webhook secret from `TROGON_GATEWAY_LOCAL_GITHUB_WEBHOOK_SECRET` and resolves it
from OpenBao instead. The examples below create that credential for `local`.

Then start the stack:

```bash
docker compose up --build --remove-orphans
```

Create a GitHub webhook secret:

```bash
curl -sS -X PUT "http://trogon-gateway.trogonai.orb.local:8080/-/credentials/github/local/webhook-secret" \
  -H "content-type: application/json" \
  -H "x-trogon-admin-token: local-admin-token" \
  -H "Idempotency-Key: local-github-local-webhook-secret-create-1" \
  -d '{"owner_id":"tenant-1","secret":"copy-this-value-in-and-out"}'
```

Rotate it:

```bash
curl -sS -X POST "http://trogon-gateway.trogonai.orb.local:8080/-/credentials/github/local/webhook-secret/rotations" \
  -H "content-type: application/json" \
  -H "x-trogon-admin-token: local-admin-token" \
  -H "Idempotency-Key: local-github-local-webhook-secret-rotate-1" \
  -d '{"owner_id":"tenant-1","version":1,"secret":"rotated-copy-this-value-in-and-out"}'
```

Revoke the rotated version:

```bash
curl -sS -X DELETE "http://trogon-gateway.trogonai.orb.local:8080/-/credentials/github/local/webhook-secret" \
  -H "content-type: application/json" \
  -H "x-trogon-admin-token: local-admin-token" \
  -H "Idempotency-Key: local-github-local-webhook-secret-revoke-1" \
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
docker compose exec -e BAO_ADDR=http://127.0.0.1:8200 -e BAO_TOKEN=local-dev-root-token openbao \
  bao kv get -format=json -mount=secret \
  "trogonai/tenant-1/credentials/openbao:tenant-1:github/local:webhook_secret"
```

Do not percent-encode this value for the `bao` CLI. The HTTP adapter URL-encodes
path segments internally when it calls OpenBao.

## Verify

Subscribe to NATS as the `PLATFORM` developer identity to see events flowing:

```bash
docker compose --profile tools run --rm nats-box nats sub '>'
```

A `nats` CLI on the host connects with the files `.env.example` shows how to
copy out of the volume:

```bash
nats --server tls://localhost:4222 --tlsca tmp/nats/ca.pem \
  --tlscert tmp/nats/client.pem --tlskey tmp/nats/client-key.pem \
  --creds tmp/nats/nats.creds sub '>'
```

## Environment variables

See `devops/docker/compose/.env.example` for gateway, NATS, logging, local
tunnel env vars, and secret values referenced explicitly from `gateway.toml`.
Source integrations are configured in TOML.

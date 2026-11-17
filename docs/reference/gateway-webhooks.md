# Generic gateway webhooks

The gateway accepts events at `POST /webhooks/{endpoint_id}` and publishes a
`trogonai.gateway.webhooks.v1.WebhookReceived` message to JetStream. Consumers
decide what to do with the delivery. The receiver has no workflow, agent,
prompt, filter, or coalescing dependency.

The reusable receiver and registry interfaces live in `trogon-webhooks`. The
gateway supplies its infrastructure configuration and credential resolver.
Provider-specific routes under `/sources/` retain their provider protocols.

## Gateway configuration

An explicit `[webhooks]` section enables generic ingress:

```toml
[webhooks]
registry_bucket = "GATEWAY_WEBHOOK_ENDPOINTS"
stream_name = "WEBHOOKS"
subject_prefix = "webhooks"
stream_max_age_secs = 604800
duplicate_window_secs = 604800
admission_timeout_secs = 10
```

These settings configure infrastructure, not individual endpoints. Endpoint
changes take effect without rebuilding the HTTP router or restarting the
gateway. The configured namespace and stream name must not collide with
provider streams or gateway credential resources.

`admission_timeout_secs` bounds lookup, body reading, verification, and broker
publication together. A timeout returns a retryable response. It does not prove
that a broker publication was absent; retry using the same delivery identifier.

Generic ingress uses the existing credential lifecycle and runtime resolver.
Its current gateway assembly requires `OPENBAO_ADDR` and `OPENBAO_TOKEN`.
Credential management additionally requires
`TROGON_GATEWAY_CREDENTIAL_MANAGEMENT_ADMIN_TOKEN`. Configure these through the
existing deployment secret mechanism. Verifier credentials are not added to
endpoint records or delivery events. Request bodies are retained unchanged in
`WebhookReceived` and may contain sensitive data.

## Endpoint registry

The default registry is NATS KV. Each key is the endpoint's canonical,
lowercase, non-nil UUID. Its value is a binary protobuf `EndpointRegistration`:

| Field | Meaning |
| --- | --- |
| `endpoint_id` | Must match the registry key and requested endpoint. |
| `project_id` | Owning project; a single bounded subject token. |
| `status` | Explicit enabled or disabled state. |
| `authentication_mode` | Explicit bearer or HMAC-SHA256 verification. |
| `max_body_bytes` | Per-endpoint payload limit, at most 1 MiB. |
| `timestamp_tolerance_seconds` | Nonzero signed-request clock tolerance, at most 3600 seconds. |

The KV entry revision becomes `configuration_revision` in the delivery event.
Deleted, purged, unknown, disabled, malformed, or mismatched records cannot admit
a delivery. The receiver reads the current entry for each request.

An update affects subsequent lookups. Requests that already resolved an earlier
revision can finish under that revision; disabling an endpoint does not retract
stored deliveries or cancel downstream work.

A control-plane integration constructs a validated
`trogon_webhooks::EndpointRegistration`, encodes it with `encode()`, and creates
the KV key. Updates use the expected KV revision. Disable an endpoint by writing
its disabled state, or delete its record. Restrict registry write access to the
control plane using NATS permissions; external webhook callers receive no NATS
access.

`EndpointLookup` is the replacement boundary for another authoritative registry,
including a database. A replacement returns a validated `RegisteredEndpoint`
with the exact configuration revision used for that admission.

## Authentication

An endpoint identifier is a locator, not an authentication credential. Each
endpoint has an integration-scoped `webhook_secret` in the existing credential
lifecycle, with source `webhook`, integration ID equal to the endpoint UUID,
and owner equal to `project_id`.

The existing admin-authenticated management paths are:

```text
PUT    /-/credentials/webhook/{endpoint_id}/webhook-secret
POST   /-/credentials/webhook/{endpoint_id}/webhook-secret/rotations
DELETE /-/credentials/webhook/{endpoint_id}/webhook-secret
POST   /-/credentials/webhook/{endpoint_id}/webhook-secret/destructions
```

They reuse the credential API's request and idempotency contracts. The runtime
resolver enforces project ownership, credential status, and configured release
policy before reading a secret or returning a cached value. Rotation and
revocation follow the existing credential projection's propagation bounds.

### Bearer requests

Send one `Authorization: Bearer ...` header and one `webhook-id` header. The
token is verified in constant time. Keep it in the sender's secret store; do
not place it in a URL or registry record.

### Signed requests

HMAC mode uses the Standard Webhooks `v1` construction:

```text
signed_bytes = webhook-id + "." + webhook-timestamp + "." + raw_body
webhook-signature = "v1," + base64(HMAC-SHA256(signing_key, signed_bytes))
```

Send one each of `webhook-id`, `webhook-timestamp`, and `webhook-signature`.
The timestamp is Unix time in seconds; requests outside the endpoint's tolerance
are refused in either clock direction. The signing secret uses a `whsec_` prefix
followed by a base64-encoded 32-byte key. Verification uses the original
request bytes, including whitespace and non-UTF-8 payloads.

For either mode, `webhook-id` must contain 1 to 128 visible ASCII characters
excluding `.`. Rejecting this separator keeps the signed ID, timestamp, and
body unambiguous; the body may still contain periods. Reuse the identifier when
retrying the same delivery. Duplicate authentication or delivery headers are
refused. Authentication headers are never forwarded to consumers.

## Delivery contract

The receiver publishes to:

```text
{subject_prefix}.{project_id}.{endpoint_id}.received
```

The project, endpoint, and configuration revision come from the validated
registry record. The caller cannot choose a NATS subject, message ID, project,
or authority through request headers or body fields.

`WebhookReceived` contains the endpoint and project identifiers, configuration
revision, delivery identifier, receipt timestamp, original body bytes, and
optional content type. It carries no workflow target or execution permissions.
Consumers subscribe to the authorized project or endpoint subjects, decode the
protobuf envelope, and treat its body as untrusted external data.

The NATS message ID derives from project, endpoint, and delivery identity.
JetStream suppresses retries within the configured duplicate window. This
provides bounded admission deduplication; consumers remain responsible for
idempotent processing and external side effects. The original stored event
retains the configuration revision under which it was accepted.

Large envelopes use the gateway's existing claim-check transport. Consumers
must resolve claim headers through the matching `trogon-nats` claim bucket
before decoding the envelope. Claim retention follows stream retention.

## Responses

| Status | Meaning |
| --- | --- |
| `202` | JetStream acknowledged the delivery, including a recognized duplicate. |
| `400` | The request metadata is malformed or ambiguous. |
| `401` | Authentication or signed-request freshness failed. |
| `404` | No enabled endpoint is available at that path. |
| `405` | The route does not accept that HTTP method. |
| `413` | The body exceeds the endpoint or receiver ceiling. |
| `503` | Registry lookup, body reading, verification, or durable publication could not complete. |

A `202` response acknowledges storage. It does not report downstream execution
or completion.

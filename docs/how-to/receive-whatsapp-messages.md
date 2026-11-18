# Receive WhatsApp messages

Use an existing Meta app with WhatsApp Business Cloud API, a Messaging account
(WABA), and a registered business phone number. The gateway turns inbound
messages for that number into durable JetStream events.

## Configure the gateway

Give the integration a name and set both Meta IDs as quoted strings. Inject the
Meta **App Secret** and a separate verification token you choose through the
gateway process environment. Put the variable names in the TOML.

```toml
[sources.whatsapp.integrations.support.webhook]
waba_id = "102290129340398"
phone_number_id = "106540352242922"
verify_token = { env = "WHATSAPP_SUPPORT_VERIFY_TOKEN" }
app_secret = { env = "WHATSAPP_SUPPORT_APP_SECRET" }
```

Use the business phone number **ID** from Meta, which identifies the destination
in `value.metadata.phone_number_id`. The WABA ID appears in `entry.id`.

Start or restart the gateway with this configuration. Expose
`/sources/whatsapp/support/webhook` through public HTTPS with a trusted
certificate. Forward GET and POST to the same path. Preserve the POST bytes and
the `X-Hub-Signature-256` header, and exclude query strings from access logs
because Meta sends the verification token in the GET query.

The default POST limit is 3,000,000 bytes. Set `max_body_bytes` in the webhook
table to lower it. Make sure the proxy allows the configured body size. The
named integration's optional `nats_ack_timeout_secs` sets the publication
budget for a batch and defaults to 10 seconds.

## Subscribe in Meta

1. In the Meta App Dashboard, enter
   `https://<gateway-host>/sources/whatsapp/support/webhook` as the callback URL.
   Use the same verification token supplied to the gateway. Meta verifies the
   URL with a GET request and expects the challenge as plain text. [Create a webhook endpoint](https://developers.facebook.com/documentation/business-messaging/whatsapp/webhooks/create-webhook-endpoint)
2. Subscribe to the `messages` webhook field for
   `whatsapp_business_account`. Ensure the app has the
   `whatsapp_business_messaging` permission and is subscribed to the target
   Messaging account. [WhatsApp webhooks](https://developers.facebook.com/documentation/business-messaging/whatsapp/webhooks/overview) · [Manage Webhooks](https://developers.facebook.com/documentation/business-messaging/whatsapp/solution-providers/manage-webhooks#subscribe-to-a-whatsapp-business-account)
3. If the app serves several WABAs or phone numbers, set Meta's account or
   phone-number callback override for each separately configured route.
   [Override the callback URL](https://developers.facebook.com/documentation/business-messaging/whatsapp/webhooks/override)
4. Send a WhatsApp message from a user account to the configured business
   number. Check that the corresponding `message.id` appears on
   `whatsapp-support.message.received` in `WHATSAPP_SUPPORT`.

## Consume messages

Each event contains `waba_id`, `phone_number_id`, and the original Meta
`message` object from `value.messages[]`. Use `message.from_user_id`, the
business-scoped user ID, when present. `message.from` can be absent. For media
messages, the event contains Meta's media reference. Large event payloads use the
gateway's JetStream Object Store claim check. [Meta's messages webhook reference](https://developers.facebook.com/documentation/business-messaging/whatsapp/webhooks/reference/messages)

The gateway verifies the signature and validates the batch before publishing.
It bounds each request to 1,000 webhook changes and 1,000 admitted messages.
A mixed batch publishes only inbound messages for the configured WABA and
business phone number. Outbound statuses and other webhook fields produce no
event. If there are no admitted messages and the batch contains a `messages`
change for another WABA or inbound messages for another phone, the gateway
returns `403`.

The gateway returns `200` after JetStream acknowledges every admitted message,
or immediately after validation when there are no messages to publish. A
publication failure returns `503` so Meta retries. Earlier publishes from the
batch may already be durable. JetStream suppresses duplicates for the same
WABA, phone number and message ID on the named stream, even if Meta changes the
batch. The duplicate window is seven days or the stream retention period,
whichever is shorter. Changing or recreating the stream can reset that history.
Consumers should keep their own idempotency state for repeated processing and
retries outside that window. Batch delivery does not guarantee event order.

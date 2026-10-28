# Receive Meeting Transcript Notifications

Use this procedure to get a NATS message every time a meeting platform
finishes a transcript, so a consumer can fetch the transcript and act on it.
`trogon-gateway` only receives the notification and publishes it to JetStream.
It never downloads the transcript; the consumer does that with its own
credentials against the platform's API.

Pick the section for the platform your meetings run on.

When an event body is larger than the NATS server's maximum payload, the
gateway stores it in the claim-check object store and publishes an empty body
with `Trogon-Claim-Check`, `Trogon-Claim-Bucket`, and `Trogon-Claim-Key`
headers instead. This applies to every platform below, so resolve the claim,
for example with `trogon_nats::jetstream::ClaimResolver`, before parsing the
body.

## Before you begin

Confirm you have:

- A running `trogon-gateway` connected to NATS JetStream. See the
  [gateway README](https://github.com/TrogonStack/trogonai/tree/main/rsworkspace/crates/platform/trogon-gateway)
  for configuration and routes.
- For Zoom and Microsoft Teams, a public HTTPS URL that reaches the gateway's
  HTTP port.
- Admin rights on the meeting platform to create an app or subscription.

## Google Meet

Google Meet delivers transcript events through the Google Workspace Events API
to a Cloud Pub/Sub topic. The gateway pulls them from a subscription on that
topic, so it needs no public URL for Meet.

1. Create a Pub/Sub topic and a pull subscription on it. Grant
   `roles/pubsub.publisher` on the topic to
   `meet-api-event-push@system.gserviceaccount.com`, or Meet cannot deliver
   events to it.
2. Create a Workspace Events subscription whose target is the Meet space or
   user you want to watch, whose event types include
   `google.workspace.meet.transcript.v2.fileGenerated`, and whose notification
   endpoint is the topic from the previous step. A user target covers only the
   meeting spaces that user owns, not meetings the user joins as a
   participant, so target the space itself to watch a meeting space someone
   else owns.
3. Give the gateway a service account that can pull from the subscription, and
   add a dedicated integration so Meet events do not share a subject with other
   Pub/Sub traffic:

   ```toml
   [sources.google_pubsub.integrations.meet.streaming_pull]
   subscription = "projects/<project>/subscriptions/<subscription>"
   credentials = { env = "GOOGLE_PUBSUB_MEET_CREDENTIALS" }
   ```

4. Consume `google-pubsub-meet.message`. The body is the event payload, which
   names the transcript resource. Each Pub/Sub attribute arrives as a NATS
   header prefixed with `X-PubSub-Attribute-`, so filter on
   `X-PubSub-Attribute-ce-type` to tell transcript events from other Meet
   events. The Pub/Sub message ID is the NATS message ID, so JetStream drops
   redeliveries that arrive within the stream's duplicate window.

Workspace Events subscriptions expire. Renew them before they lapse, or Meet
stops sending events without an error the gateway can see.

## Zoom

Zoom only produces transcripts for meetings recorded to the cloud with audio
transcription turned on. Confirm the host's account has a license that
includes cloud recording and that both Cloud recording and Audio transcript
are enabled; otherwise the steps below succeed but no transcript event is ever
sent.

1. In a Zoom app, enable Event Subscriptions, subscribe to
   `recording.transcript_completed`, and set the endpoint URL to
   `https://<gateway-host>/sources/zoom/<integration>/webhook`.
2. Add the app's Secret Token to the gateway:

   ```toml
   [sources.zoom.integrations.primary.webhook]
   secret_token = { env = "ZOOM_PRIMARY_SECRET_TOKEN" }
   ```

3. Start the gateway, then validate the endpoint in Zoom. The gateway answers
   the `endpoint.url_validation` handshake itself.
4. Consume `zoom-primary.recording.transcript_completed`. The body is Zoom's
   event payload, which carries the recording files and their download URLs.

Zoom sends no stable delivery identifier, so its retries reach NATS as separate
messages. Make the consumer idempotent, for example by keying on the meeting
UUID and the transcript file ID in the payload.

## Microsoft Teams

Teams transcripts are announced through Microsoft Graph change notifications.

1. Add the Graph integration to the gateway with a `client_state` value you
   generate:

   ```toml
   [sources.microsoft_graph.integrations.primary.webhook]
   client_state = { env = "MICROSOFT_GRAPH_PRIMARY_CLIENT_STATE" }
   ```

2. Create a Graph subscription on an online meeting transcripts resource,
   such as `communications/onlineMeetings/getAllTranscripts` for the whole
   tenant or `users/{userId}/onlineMeetings/getAllTranscripts` for one
   organizer, with `changeType` `created`, `notificationUrl` set to
   `https://<gateway-host>/sources/microsoft-graph/primary/webhook`, and the
   same `clientState`. The gateway answers Graph's validation request when the
   subscription is created.

   If `expirationDateTime` is more than one hour away, Graph also requires
   `lifecycleNotificationUrl`. Pointing it at the same gateway URL works: the
   gateway answers its validation request and publishes lifecycle
   notifications to the same subject, where consumers tell them apart by the
   `lifecycleEvent` field that transcript notifications do not carry.
3. Consume `microsoft-graph-primary.change_notification_collection`. Each
   notification names the transcript resource; fetch its content through
   Graph.

Graph only announces a transcript when the subscription already exists before
transcription starts, and these resources do not cover private channel
meetings. Graph subscriptions also expire after a short, resource-specific
lifetime. Renew them on a schedule, or notifications stop.

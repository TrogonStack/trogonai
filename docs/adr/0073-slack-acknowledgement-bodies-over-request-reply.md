---
number: "0073"
slug: slack-acknowledgement-bodies-over-request-reply
status: draft
date: 2026-10-25
---

# ADR#0073: A Slack Acknowledgement Body Comes From a Consumer Over NATS Request/Reply, After the Durable Publish

## Context

`trogon-gateway` receives Slack deliveries over HTTP and Socket Mode, publishes
each one to JetStream, and acknowledges Slack with an empty body. Slack expects
the acknowledgement within 3 seconds, and for a few deliveries it reads the
body of that acknowledgement and acts on it
([Slack Acknowledgement Responses](../architecture/slack-acknowledgement-responses.md)):

| Delivery | What the body does |
| -------- | ------------------ |
| `view_submission` | `response_action` keeps the modal open with `errors`, replaces it with `update`, stacks a view with `push`, or closes the stack with `clear`. An empty body closes the submitted view, and no Web API method can do what the body does. |
| `block_suggestion` | The body is the list of `options` for an external select. There is no other channel for it. |
| Slash command | The body is an immediate message. The `response_url` can send the same message, so the body is a convenience. |

Every other delivery ignores the body. Bolt encodes this in its types:
`AckFn<void>` for events, actions and shortcuts; `AckFn<ViewResponseAction>`,
`AckFn<BlockSuggestionResponse>` and `AckFn<string | RespondArguments>` for the
three above. Its receivers write a string body as `text/plain` and an object as
`application/json` over HTTP (`HTTPModuleFunctions.ts` `buildContentResponse`),
and over Socket Mode send `{ envelope_id, payload }`, wrapping a string as
`{ "text": ... }` (`SocketModeClient.ts` `send`). When no listener acknowledges
within 3001 ms Bolt answers HTTP with `404` and sends no Socket Mode frame
(`HTTPModuleFunctions.ts` `defaultUnhandledRequestHandler`), so Slack shows the
user an error and, over Socket Mode, redelivers the envelope.

The gateway is meant to replace the Bolt receiver with full behavioral parity
and to drop nothing Slack sends. A one-way publish cannot produce the body,
because the only process that knows what to tell Slack is the consumer
application, and Slack holds the request or envelope open on a connection only
the gateway owns.

Two properties of NATS constrain the shape of the answer:

1. A JetStream stream captures every message published on a subject it
   filters, including a core NATS request. A request subject that falls under
   the stream filter is persisted whether anyone meant it to be or not.
2. A JetStream consumer is not waiting on anything. A reply-to header on a
   stored message puts every consumer of the stream on the 3 second clock, and
   a redelivery after the deadline replies to a request that no longer exists.

## Decision

### 1. Durable publish first, then a request, for the deliveries that honor a body

For `view_submission` and slash commands the gateway publishes to JetStream
exactly as it does today, and only after the stream acknowledges does it send
a core NATS request and wait for a reply. The reply becomes the body of the
acknowledgement to Slack. For every other delivery the gateway keeps publishing
and acknowledging with an empty body.

Publish failure is handled as it is today: `500` over HTTP, no acknowledgement
over Socket Mode, and no request is sent. A delivery that reached JetStream and
then timed out on the request is a stored delivery with a failed
acknowledgement, never a lost one.

### 2. `block_suggestion` is a request and nothing else

`block_suggestion` carries no `trigger_id`, fires on every keystroke, and its
only purpose is the reply. It is never published to JetStream. This is the one
Slack delivery the gateway does not store, recorded here as a deliberate
exception to the verbatim transport rule.

### 3. Respond subjects mirror the publish subjects behind a direction token

The publish subjects are `{prefix}.event.{type}`, `{prefix}.interaction.{type}`
and `{prefix}.command.{name}`. Requests use the same shape under a fixed
`respond` token:

```
{prefix}.respond.interaction.view_submission
{prefix}.respond.interaction.block_suggestion
{prefix}.respond.command.{name}
```

Subscriber paths:

```
{prefix}.respond.>                 every request for one integration
{prefix}.respond.interaction.*     modal submissions and option lookups
{prefix}.respond.command.*         every slash command
```

Responders subscribe with a queue group of their own choosing; the gateway does
not know or care how many there are.

Developer-chosen identifiers such as the view `callback_id`, the `action_id`
and the `block_id` are not subject segments. Slack lets them hold dots, spaces
and wildcard characters, which would break the subject or let a value steer a
request to another subject. They travel as headers, `X-Slack-Callback-Id`,
`X-Slack-Action-Id` and `X-Slack-Block-Id`, and a responder matches on them in
process, which is where Bolt matches them too.

### 4. The request carries the delivery, the reply carries the body

The request body is the Slack payload byte for byte, and the headers are the
headers of the published message, including `Nats-Msg-Id`, so a responder can
correlate the request with the stored record without a lookup.

The reply body is forwarded to Slack byte for byte. A reply with a
`Content-Type` header of `text/plain` is a slash command text reply; any other
reply is JSON. Over HTTP the body and the content type go on the `200`
response. Over Socket Mode the body becomes `payload` of the acknowledgement
frame, and a text reply is wrapped as `{ "text": ... }`, which is Bolt's
encoding. An empty reply is an explicit empty acknowledgement.

When a Socket Mode envelope sets `accepts_response_payload` to `false`, Slack
would discard the body, so the gateway sends no request and acknowledges with
no body.

### 5. The deadline is Slack's, and nothing else

The gateway holds one deadline per delivery, 3 seconds from receipt. The
publish gets at most 1 second of it, and the request gets what remains after a
200 millisecond margin for writing the acknowledgement. No timeout on the
acknowledgement path is configured independently, and none exceeds the
deadline. The events path keeps the same deadline, since a publish Slack has
already given up waiting for only earns a retry into the deduplication window.

### 6. No reply within the deadline is a failed acknowledgement

A request that times out, or that NATS answers with no responders, fails the
acknowledgement the way Bolt does: `404` over HTTP, no frame over Socket Mode.
The gateway never substitutes an empty body for a missing reply, because an
empty body is itself a meaningful answer for a modal, and because a silent
substitution hides a responder outage from the only person who can notice it.

### 7. The gateway does not record replies

What a responder answers is the responder's output, not an inbound delivery.
A responder that wants a record of its replies publishes them itself. The
gateway logs the delivery kind, subject and outcome and nothing more.

### 8. The stream filters stay off the respond subjects

Because a stream captures every message on its subjects, the Slack stream is
provisioned on `{prefix}.event.>`, `{prefix}.interaction.>`,
`{prefix}.command.>` and `{prefix}.unroutable` rather than `{prefix}.>`, and
provisioning reconciles the subject list of an existing stream through
`create_or_reconcile_stream` instead of leaving a live stream on the broad
filter.

## Alternatives Rejected

- **No body; document the Web API alternatives.** Rejected because inline
  modal validation, `clear`, and external select options have no alternative,
  and the gateway would be unable to replace Bolt for any app that uses them.
- **Fixed bodies from configuration**, for example `update` to a declared
  "processing" view per `callback_id`. Deterministic and consumer-free, but it
  covers none of the cases above, and the configuration would become an
  application in the gateway's config file.
- **A reply-to header on the stored message.** One message instead of two, but
  it puts every JetStream consumer on the acknowledgement clock and makes a
  redelivery reply to nobody.
- **A respond subject keyed by `callback_id` or `action_id`.** Finer wildcard
  routing, at the price of letting a developer-chosen string shape the subject.
  Headers keep the subject fixed and the matching where Bolt already does it.
- **Empty acknowledgement on timeout.** Closer to "always answer Slack", but
  not what Bolt does, and for a modal the empty body is an action in its own
  right.
- **A separate outbound HTTP callback from the gateway to the consumer.** Adds
  a second transport and a second set of credentials for one feature that NATS
  request/reply already provides.

## Consequences

- The acknowledgement path gains a synchronous dependency on a consumer for
  three delivery kinds. A consumer that does not use modals, external selects
  or slash command bodies subscribes to nothing new and is unaffected.
- A responder sees a `view_submission` or a slash command twice: once as the
  request, once on its JetStream consumer. It must be idempotent, which Slack's
  own retries already require.
- Interactions without a `trigger_id` must be accepted, the acknowledgement
  path must fit the deadline, and Socket Mode must handle envelopes
  concurrently so one slow responder does not block the acknowledgements of
  unrelated envelopes. These land before this decision is implemented.
- Existing Slack streams provisioned on `{prefix}.>` get their subject list
  narrowed on the next gateway start. Messages already stored are unaffected.
- `block_suggestion` deliveries are not replayable. An audit that needs them is
  the responder's to produce.
- The published architecture page is updated to describe the chosen mechanism
  rather than options.

## References

- [Slack Acknowledgement Responses](../architecture/slack-acknowledgement-responses.md)
- [Slack: handling user interaction](https://docs.slack.dev/interactivity/handling-user-interaction)
- [Slack: modals](https://docs.slack.dev/surfaces/modals)
- [Slack: using Socket Mode](https://docs.slack.dev/apis/events-api/using-socket-mode)
- [Slack: implementing slash commands](https://docs.slack.dev/interactivity/implementing-slash-commands)
- [bolt-js `HTTPModuleFunctions.ts`](https://github.com/slackapi/bolt-js/blob/3f8823c93f5639e99f05a2df9c7dc692da5cc184/src/receivers/HTTPModuleFunctions.ts)
- [bolt-js `types/actions/index.ts`](https://github.com/slackapi/bolt-js/blob/3f8823c93f5639e99f05a2df9c7dc692da5cc184/src/types/actions/index.ts#L75-L79)
- [node-slack-sdk `SocketModeClient.ts`](https://github.com/slackapi/node-slack-sdk/blob/d02afdfd5c87f896dc9994dfe2b55472758fbcf4/packages/socket-mode/src/SocketModeClient.ts)
- [ADR#0044](./0044-inbound-media-fetch-out-of-band.md)

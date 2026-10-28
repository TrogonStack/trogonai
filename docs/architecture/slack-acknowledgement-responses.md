# Slack Acknowledgement Responses

Slack expects every delivery to be acknowledged within 3 seconds. Over HTTP
the acknowledgement is the response to the request; over Socket Mode it is a
frame carrying the envelope's `envelope_id`. For a few interactions the
acknowledgement can also carry a body, and Slack acts on it. `trogon-gateway`
acknowledges after the delivery is durably published to
[JetStream](../glossary/jetstream) and never carries a body, so any Slack
feature that exists only in that body is unavailable to a consumer behind the
gateway. This page records which features those are, what the Web API offers
instead, and how the gateway will carry the rest.

Legacy surfaces (dialogs, attachment-based `interactive_message`, workflow
steps from apps) are out of scope.

## Where Slack ignores the body

For most deliveries the acknowledgement is only a receipt. Bolt types these
acknowledgements as taking no value, and states it for actions: "message
action and block actions don't accept any value in the ack response"
([bolt-js `types/actions/index.ts`](https://github.com/slackapi/bolt-js/blob/3f8823c93f5639e99f05a2df9c7dc692da5cc184/src/types/actions/index.ts#L75-L79)).
The consumer responds through the Web API.

| Delivery | How a consumer responds |
| -------- | ----------------------- |
| Events API events | Web API methods such as `chat.postMessage` or `views.publish` |
| `function_executed` (custom workflow steps) | `functions.completeSuccess` or `functions.completeError` |
| `link_shared`, `entity_details_requested` (Work Objects) | `chat.unfurl`, `entity.presentDetails` with the event's `trigger_id` |
| `block_actions` in messages, modals, and App Home | `response_url`, `chat.update`, or `views.update` / `views.push` with the payload's `trigger_id` |
| Global and message shortcuts | `views.open` with the `trigger_id`; message shortcuts also carry a `response_url`, global shortcuts do not |
| `view_closed` | Nothing; the modal is already closed |

A `trigger_id` expires 3 seconds after Slack sends it and can be used once
([Slack: handling user interaction](https://docs.slack.dev/interactivity/handling-user-interaction)).
A consumer that opens or pushes a modal therefore has to get from the gateway
to the Slack API within that window, whether or not the gateway ever carries a
body. A `response_url` accepts up to 5 posts within 30 minutes.

## Where the body carries meaning

Socket Mode carries the same body inside the acknowledgement frame as
`{ "envelope_id": ..., "payload": ... }`, and marks the envelopes that accept
one with `accepts_response_payload`
([Slack: using Socket Mode](https://docs.slack.dev/apis/events-api/using-socket-mode)).

### Modal submissions

A `view_submission` acknowledgement may carry a `response_action`. An empty
acknowledgement "will immediately close the submitted view and remove it from
the view stack", and an app "cannot use any other method to close views"
([Slack: modals](https://docs.slack.dev/surfaces/modals)).

| `response_action` | Effect | Without the body |
| ----------------- | ------ | ---------------- |
| `errors` | Keeps the modal open with a message under each named input | No equivalent. Input constraints Slack enforces itself, such as `min_length` and `max_length` on a plain-text input, cover simple rules. Anything else is validated after the modal has closed and reported with `chat.postEphemeral` or a new modal. |
| `update` | Replaces the submitted view in place | No equivalent. `views.update` needs a view that is still open. The asynchronous pattern still needs a body, though a fixed one: acknowledge with `update` to a "processing" view, then call `views.update` once the result is ready. |
| `push` | Pushes a new view on top of the submitted one | Partial. `views.push` needs a `trigger_id` from an interactive component inside the modal, which a submission is not. `views.open` with the submission's `trigger_id` shows the same content as a new modal, without the stack to go back to. |
| `clear` | Closes every view in the stack | No equivalent. An empty acknowledgement closes only the submitted view. |

### External select options

An `external_select` or `multi_external_select` asks the app for its options
with a `block_suggestion` payload. The acknowledgement must be a JSON body
holding `options`, or `option_groups`, up to 100 entries. Over HTTP Slack sends
it to the app's separate Options Load URL
([Slack: select menu element](https://docs.slack.dev/reference/block-kit/block-elements/select-menu-element)).

Live typeahead over external data has no equivalent without the body. The
substitutes trade it away:

- A `static_select` with up to 100 options fixed when the view or message is
  built.
- Slack's own sources: `users_select`, `conversations_select`,
  `channels_select`.
- A search input with `dispatch_action_config`, whose `block_actions` the
  consumer answers by rebuilding a `static_select` through `views.update` or
  `chat.update`.

### Slash command replies

A slash command acknowledgement may carry a message, as text or JSON with
`blocks` and a `response_type` of `ephemeral` (the default) or `in_channel`
([Slack: implementing slash commands](https://docs.slack.dev/interactivity/implementing-slash-commands)).
The command's `response_url` sends the same messages with the same
`response_type`, so a consumer loses nothing practical. Slack documents that
an `in_channel` reply in the acknowledgement also shows the user's original
command in the channel; whether a `response_url` reply does the same is not
documented.

## Summary

| Need | Requires the body | Closest alternative |
| ---- | ----------------- | ------------------- |
| Inline validation errors in a modal | Yes | Input constraints Slack enforces; validation after close |
| Close the whole modal stack on submit | Yes | None |
| Replace the submitted view | Yes | Fixed "processing" `update`, then `views.update` |
| Push onto the modal stack on submit | Yes, to keep the stack | `views.open` with the submission's `trigger_id` |
| External select typeahead | Yes | Static selects, Slack's own sources, search input |
| Slash command reply | No | `response_url` |
| Everything else | No, the body is ignored | Web API |

## Gateway Today

Both transports acknowledge only after the publish to JetStream succeeds:
`server.rs` answers HTTP with an empty `200`, and `socket_mode.rs` sends a
frame holding only `envelope_id`. That is complete for every delivery in
[Where Slack ignores the body](#where-slack-ignores-the-body) and for slash
commands.

Two more restrictions apply before the body even matters. The gateway uses an
interaction's `trigger_id` as the [JetStream](../glossary/jetstream)
deduplication ID and rejects interactions without one with `400`, publishing
them only to the `unroutable` subject. `block_suggestion` and `view_closed`
carry no `trigger_id`, so neither reaches a consumer today, and over Socket
Mode the missing acknowledgement makes Slack redeliver. The publish timeout
(`nats_ack_timeout_secs`, default 10 seconds) is also longer than Slack's
3-second deadline, so a slow JetStream acknowledgement can make Slack report a
failure for a delivery that was published.

## Direction

[ADR#0073](../adr/0073-slack-acknowledgement-bodies-over-request-reply.md)
records the decision. The gateway will carry the body for the deliveries that
honor one:
`view_submission`, `block_suggestion`, and slash commands. Slash commands are
included for parity with Bolt's `ack(text)`, not because the Web API lacks an
equivalent.

For those deliveries the gateway publishes to JetStream as it does today, then
sends a NATS request on a respond subject and waits for a consumer's reply
within a budget under 3 seconds. The reply becomes the HTTP response body, or
the `payload` of the Socket Mode acknowledgement frame when the envelope sets
`accepts_response_payload`. On timeout, or when no consumer is subscribed, the
gateway fails the acknowledgement the way Bolt does: HTTP answers `404` and
Socket Mode sends no acknowledgement frame
([bolt-js `HTTPModuleFunctions.ts`](https://github.com/slackapi/bolt-js/blob/3f8823c93f5639e99f05a2df9c7dc692da5cc184/src/receivers/HTTPModuleFunctions.ts#L227-L239)).
Slack reports the failure to the user and, over Socket Mode, redelivers. The
delivery is already in JetStream by then, so nothing is lost.

The durable publish and the request answer different questions. The publish
records that the delivery happened, for any consumer at any later time. The
request asks one responder what to tell Slack right now, and nobody can answer
it later because Slack holds the connection open for 3 seconds at most. A
reply-to header on the JetStream message was rejected: it would put every
consumer of the stream on that 3-second clock, and a redelivery after the
deadline would reply to nobody.

`block_suggestion` is a query with no stable identifier that fires on every
keystroke, so it is sent as a request only and never stored. The ADR records
that as the one exception to the verbatim transport rule, fixes the respond
subjects and the deadline, and leaves recording replies to the consumer.
Accepting interactions without a `trigger_id` and keeping the acknowledgement
path under 3 seconds are prerequisites.

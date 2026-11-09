---
number: "0080"
slug: slack-bolt-nats-receiver
status: draft
date: 2026-10-29
---

# ADR#0080: A Bolt for JavaScript App Consumes the Gateway's Slack Traffic Through a `NatsReceiver` That Dispatches What the Gateway Already Verified and Stored

## Context

`trogon-gateway` is the only process that talks to Slack. It verifies the
signature, answers `url_verification` and `ssl_check`, owns the Socket Mode
connection, publishes each delivery to JetStream, and acknowledges Slack
([ADR#0073](./0073-slack-acknowledgement-bodies-over-request-reply.md) adds a
NATS request for the few deliveries whose acknowledgement carries a body). What
it leaves behind is a stream of verified deliveries and a set of request
subjects. Nothing in the repository consumes them on behalf of an application
that is already written against Bolt for JavaScript.

Such applications exist, and rewriting their listeners is the cost that stops
them from moving behind the gateway. Bolt isolates exactly this seam. A
receiver is the object that turns an incoming transport into calls to
`app.processEvent(event)` and gives each call an `ack` function. Bolt's
`Receiver` interface is `init(app)`, `start()` and `stop()`, and a
`ReceiverEvent` carries `body`, `retryNum`, `retryReason`, `customProperties`
and `ack` (`types/receiver.ts`). `HTTPReceiver` and `SocketModeReceiver` are two
implementations of it. Everything above the seam, which is the listeners,
middleware, `say`, `respond`, `client` and the error handler, does not know
which one is in use.

These facts about Bolt and about what the gateway publishes shape the answer.

1. **Bolt acknowledges Events API events before any listener runs.**
   `processEvent` calls `await ack()` itself for every event except
   `function_executed`, then runs the listeners. For interactions and commands
   the listener calls `ack` and usually does its real work afterwards. So `ack`
   is where Slack's acknowledgement ends, not where the application's work ends.
2. **Listener errors do not reach the receiver.** Bolt sends them to the app's
   error handler, which by default logs and returns, and `processEvent` then
   resolves. It rejects only when that handler itself throws, or for an invalid
   custom property. `SocketModeReceiver` already treats that rejection through
   a `processEventErrorHandler` that decides whether to acknowledge anyway.
3. **The gateway already acknowledged Slack for every stored delivery.** A
   JetStream consumer is not on Slack's clock, and a listener failure cannot
   cause a Slack retry. The retry layers are separate: Slack retries until the
   gateway's publish is stored, and the stream's duplicate window removes the
   repeats by `Nats-Msg-Id`.
4. **Some deliveries exist twice.** `view_submission` and slash commands are
   stored and also sent as a request. `block_suggestion` is only a request.

The request subjects and the stream are the gateway's, and this ADR uses them
as they are. The gateway's subject prefix is a single NATS token that defaults
to `slack-{integration_id}`, and its stream defaults to
`SLACK_{INTEGRATION_ID}`. Both are configurable, so the receiver takes them as
configuration and does not derive them.

## Decision

### 1. The receiver is a `NatsReceiver` that implements Bolt's `Receiver` and lives beside the fixtures

A TypeScript package, `tsworkspace/packages/slack-bolt-nats-receiver`,
introduced by this ADR, exports `NatsReceiver`. It implements `init(app)`,
`start()` and `stop()` and turns every Slack delivery that arrives on NATS into
one `app.processEvent(event)` call. An existing application changes only its
construction:

```ts
new App({ receiver: new NatsReceiver({ connection, prefix, stream, app: "billing-bot" }), authorize })
```

Its listeners, middleware and error handler stay as they are. The package
declares `@slack/bolt` as a peer dependency pinned to the version the
conformance fixtures were exported from, so a Bolt upgrade is a deliberate
change that re-exports them.

The receiver is given a connected NATS client and never closes it. Credentials,
reconnects and TLS are the application's, the same way the gateway's are its
own.

`start()` resolves once the consumer and the request subscriptions are active.
`stop()` stops pulling and unsubscribes, waits for `processEvent` calls already
running up to a bound the application sets, and leaves any delivery still
unsettled to redeliver.

### 2. A NATS message becomes a `ReceiverEvent` by a fixed mapping

| `ReceiverEvent` field | Source |
| --------------------- | ------ |
| `body` | The message payload, after any claim check is redeemed (section 7). Parsed by `Content-Type`: `application/json` with `JSON.parse`, and `application/x-www-form-urlencoded` with `querystring.parse`, which is what Bolt's `parseHTTPRequestBody` does. The gateway stores an HTTP slash command as the form it received and every other delivery as JSON, and an HTTP interaction is the inner `payload` JSON, so `body` equals what `HTTPReceiver` and `SocketModeReceiver` would hand Bolt. |
| `retryNum` | `X-Slack-Retry-Num` as an integer. Absent or not an integer is `undefined`. |
| `retryReason` | `X-Slack-Retry-Reason`. Absent is `undefined`. |
| `customProperties` | A `nats` object holding the subject, `Nats-Msg-Id` and, for a stored delivery, its delivery count, merged with whatever the application's `customPropertiesExtractor` returns for the message. A key that Bolt reserves for the context makes Bolt throw `InvalidCustomPropertyError`, as it does for the other receivers. |
| `ack` | Section 3 for stored deliveries and section 4 for requests. |

The receiver reads the subject to tell the kind of delivery apart. After the
configured prefix come `event`, `interaction`, `command`, `respond`, `system` or
`unroutable`. It does not read `X-Slack-Event-Type`, `X-Slack-Team-Id`,
`X-Slack-Enterprise-Id`, `X-Slack-Is-Enterprise-Install`, `X-Slack-Api-App-Id`,
`X-Slack-Transport`, `X-Slack-Envelope-Id` or `X-Slack-Request-Timestamp`.
Bolt derives team, enterprise and conversation from `body`, and these headers
are routing metadata that an extractor may still read.

Over Socket Mode Slack sends `retry_attempt: 0` on a first delivery, and
`SocketModeReceiver` passes `0` on as `retryNum`. The gateway publishes
`X-Slack-Retry-Num` only for a non-zero attempt, so the receiver reports
`undefined` where `SocketModeReceiver` reports `0`. A listener that tests
`retryNum` for truthiness behaves identically. The receiver does not invent a
`0`.

### 3. A stored delivery is acknowledged to JetStream when `processEvent` settles, and `ack()` is bookkeeping

For an event, an interaction or any other delivery that came from the durable
consumer, Slack has already been acknowledged by the gateway. The `ack` handed
to Bolt records that it was called and raises Bolt's `ReceiverMultipleAckError`
on a second call, as `HTTPReceiver` does. A body passed to it is discarded,
because there is no open request to carry it.

The JetStream message is settled by what `processEvent` does, not by `ack()`.
Because Bolt calls `ack()` for an event before any listener runs, tying the
JetStream acknowledgement to it would mark a delivery done at the moment its
work begins, and a crash then loses a delivery the stream was holding.

| `processEvent` outcome | JetStream | Why |
| ---------------------- | --------- | --- |
| Resolves | `ack` | Bolt has sent listener errors to the application's error handler. That is the outcome an HTTP or Socket Mode app produces, and Slack would not have redelivered it either. |
| Rejects and `processEventErrorHandler` returns `true` | `ack` | The handler declared the delivery handled. |
| Rejects and `processEventErrorHandler` returns `false` | `nak` with a delay that grows with the delivery count | The failure may be transient. The consumer's `max_deliver` bounds the retries, and its max deliveries advisory is the dead letter signal. |
| Rejects with `InvalidCustomPropertyError` | `term` | A redelivery produces the same error. |
| Payload does not parse, or a claim names a bucket the receiver does not read, or the claim has no key | `term` | No retry can fix it. |
| The claimed object is missing or the object store fails | `nak` | The object store may recover. |
| A message the receiver does not dispatch (section 5) | `ack` | There is nothing for Bolt to do with it. |

`processEventErrorHandler` has the signature and the default of the option of
the same name on `SocketModeReceiver`: it receives `{ error, logger, event }`
and returns whether to acknowledge, and its default returns `false` except for
`AuthorizationError`.

While `processEvent` is pending the receiver calls `working()` on the message
at half the consumer's `ack_wait`, so a listener that runs longer than
`ack_wait` is not redelivered underneath itself. An optional
`maxProcessingTime` bounds this. Past it the receiver stops extending, and the
message redelivers to any instance. By default there is no bound, which matches
a Bolt listener that never returns.

JetStream delivery is at least once. A listener can see the same
`Nats-Msg-Id` again after a crash, a `nak` or a lapsed `ack_wait`, and
`context.nats` carries the id and the delivery count so a listener that needs
idempotency can use them. The receiver does not deduplicate, and neither does
Bolt.

The receiver runs deliveries concurrently up to the consumer's
`max_ack_pending`, as an HTTP server does. Order across messages is not
preserved.

### 4. A request is answered from `ack(body)`, and a missing `ack` is a missing answer

A message on a respond subject is one that the gateway is holding Slack's
acknowledgement for. The receiver builds the same `ReceiverEvent` and its `ack`
publishes one reply to the message's reply subject:

| `ack(...)` argument | Reply payload | Reply `Content-Type` header |
| ------------------- | ------------- | --------------------------- |
| Nothing, `undefined`, `null` or an empty string | Empty | None |
| A string | The string as UTF-8 | `text/plain` |
| An object | `JSON.stringify` of it | `application/json` |

That is Bolt's own rule in `buildContentResponse`, and it is the shape the
gateway expects from [ADR#0073](./0073-slack-acknowledgement-bodies-over-request-reply.md):
the reply body is forwarded byte for byte, `text/plain` marks a slash command
text reply and is what lets the gateway wrap it as `{ "text": ... }` over
Socket Mode, and an empty reply is an explicit empty acknowledgement, which for
a modal closes the view. A second call to `ack` raises
`ReceiverMultipleAckError`.

The receiver sets `text/plain` on a string reply for every transport. Over HTTP
Bolt sends a string with no content type at all, and the conformance harness
accepts `text/plain` where a fixture names none.

The deadline belongs to Slack and the gateway holds it. For `view_submission`
and slash commands the reply must arrive within 2.8 seconds counted from the
start of the gateway's publish, and for `block_suggestion` within 2.8 seconds of
receipt. The publish and the NATS hops come out of the same budget, so the
receiver has less time than Bolt's HTTP receiver had. The receiver has no way to
extend the deadline and does not try to. It logs a warning once when a request is
still unanswered after that limit, which is the same signal Bolt's
unhandled request handler gives at 3001 ms, and it still publishes a late reply
because the reply costs nothing and an unheard inbox drops it.

A request has no JetStream message, so it has no redelivery. When a listener
never calls `ack`, the gateway receives no reply and answers Slack with a
failed acknowledgement, which is what `HTTPReceiver` does at 3001 ms. When
`processEvent` rejects before `ack` was called, `processEventErrorHandler`
returning `true` sends an empty `ack()`, as `SocketModeReceiver` does, and
`false` sends nothing. A message on a respond subject that has no reply subject
is logged and dropped.

### 5. One dispatch per delivery: the durable consumer skips what a request will carry

`view_submission` and slash commands exist as a stored message and as a request.
The receiver dispatches them from the request only, because that is the one
that can carry the acknowledgement body, and because dispatching both would run
the listener twice for one user action. A slash command that created a ticket
would create two.

| Subject | Where it comes from | Dispatched |
| ------- | ------------------- | ---------- |
| `{prefix}.event.{type}` | Durable consumer | Yes |
| `{prefix}.interaction.{type}`, except `view_submission` | Durable consumer | Yes |
| `{prefix}.interaction.view_submission` | Durable consumer | No, acknowledged and skipped |
| `{prefix}.command.{name}` | Not in the consumer's filters | Not delivered |
| `{prefix}.respond.interaction.view_submission` | Queue subscription | Yes |
| `{prefix}.respond.interaction.block_suggestion` | Queue subscription | Yes |
| `{prefix}.respond.command.{name}` | Queue subscription | Yes |
| `{prefix}.system.app_rate_limited`, `{prefix}.unroutable` | Not in the consumer's filters | Not delivered |

When the application is down for a request, the user sees Slack's own failure
and the request is not dispatched later. That is what Bolt over HTTP does when
its server is down, and a late run of a slash command the user has already seen
fail is worse than none. The stored copy remains the gateway's record, and an
operator can replay it deliberately.

`system.*` and `unroutable` stay out because Bolt has no listener for them: an
`app_rate_limited` body has no type Bolt recognizes, and an unroutable body is
by definition not a Slack payload. An application that wants them runs its own
consumer on those subjects.

### 6. Topology: a durable pull consumer per app, and a queue group named after it

The `app` name is the durable consumer name and the queue group name, so every
instance of one application shares both.

- **Stored deliveries.** A durable pull consumer on the integration's stream
  with explicit acknowledgement and the filter subjects `{prefix}.event.>` and
  `{prefix}.interaction.>`. Instances of the application pull from the same
  consumer, so each delivery goes to one of them.
- **Requests.** Core subscriptions with the queue group on
  `{prefix}.respond.interaction.*` and `{prefix}.respond.command.*`, so each
  request goes to one instance. Exactly one application should answer each
  respond subject of an integration, because the gateway forwards the first
  reply it gets and a second application with its own queue group would race
  the first.
- **Creation.** `start()` binds to the consumer and creates it when absent with
  `deliver_policy` `new`, `ack_wait` 30 seconds and `max_deliver` 5. An existing
  consumer is used as it is, so an operator who tunes `ack_wait`, `max_deliver`
  or `max_ack_pending` is not reverted, the same stance the gateway takes on a
  stream's out of band settings. `deliver_policy` `new` is deliberate: `all`
  would replay up to the stream's `max_age`, 7 days by default, into an
  application that has never seen those events.
- **Cutover.** A migrating application starts its receiver before Slack is
  pointed at the gateway, so the consumer exists when the first delivery is
  stored.
- **One integration per receiver.** A Slack app is one integration with one
  prefix and one stream. An application that serves several Slack apps runs one
  `App` and one receiver for each.

### 7. A claimed payload is redeemed before it is parsed

The gateway's publisher moves a body larger than the NATS max payload into an
object store and publishes `Trogon-Claim-Check: v1`, `Trogon-Claim-Bucket` and
`Trogon-Claim-Key` in its place
([ADR#0044](./0044-inbound-media-fetch-out-of-band.md)). A consumer that does
not redeem the claim gets an empty body and no error. The receiver therefore
takes an object store binding for the bucket, checks the bucket header against
it, fetches the key, and only then parses. The respond requests are not claim
checked: the gateway sends their body inline.

### 8. What the receiver does not do

| Concern | Owner | Where |
| ------- | ----- | ----- |
| Signature and timestamp verification | Gateway | `signature.rs`, `X-Slack-Signature`, `X-Slack-Request-Timestamp` |
| `url_verification` challenge | Gateway | Answered inline in `server.rs`, never published |
| `ssl_check` | Gateway | `ssl_check.rs`, answered before verification, never published |
| Socket Mode connection, `apps.connections.open`, ping and reconnect | Gateway | `socket_mode.rs` |
| HTTP body size limit | Gateway | `max_body_bytes` |
| Slack acknowledgement and its deadline | Gateway | Done after the publish, and over the request for the deliveries in section 5 |
| Slack retries and their deduplication | Gateway and JetStream | `Nats-Msg-Id` within the stream's duplicate window |

The receiver trusts a message on the stream because only the gateway can
publish there, so it does not hold the signing secret or the app token.
Publishing to the integration's subjects is a NATS permission, and an
application's NATS credentials should be subscribe only on them.

The receiver has no HTTP server. Bolt's `customRoutes` and the OAuth install
and redirect routes that `HTTPReceiver` and `SocketModeReceiver` can mount are
not available on it, so an application that uses them serves them from a
separate process. How the application obtains tokens, meaning `authorize`, a
`token`, or an installation store, and its Web API client are Bolt `App`
configuration and are out of scope. They are separate work.

### 9. The receiver is tested against the same Bolt fixtures as the gateway

`tsworkspace/packages/slack-bolt-fixtures` exports each fixture from a real Bolt
checkout, and the gateway replays them. A fixture records what Bolt
dispatched (`bolt.dispatch`: `incoming_type`, `conversation_id`, `retry_num`,
`retry_reason`, `is_enterprise_install`, `team_id`, `enterprise_id`,
`user_id`), the NATS message the gateway must publish (`expected.nats`: subject
suffix, headers, payload) and, where the gateway makes a request,
the request and the reply Bolt's ack produced (`expected.request`). The receiver
is the consumer half of the same contract:

- For every fixture whose `expected.outcome` is `publish` or `request`, the
  harness publishes `expected.nats`, or sends `expected.request`, to a real NATS
  server with the configured prefix, with a real Bolt `App` using the
  `NatsReceiver` and a probe in front of the listeners. The probe records the
  same dispatch record, and the test asserts it equals `bolt.dispatch`. The
  `http` and `socket_mode` fixtures of one payload must give the same record,
  because after the gateway the transport is only a header.
- For a request fixture the listener calls `ack` with the body the fixture was
  built from, and the test asserts the reply's body and content type equal
  `expected.request.reply`, with the `text/plain` allowance in section 4.
- Fixtures whose outcome is `inline`, `reject` or `ignore`, which are
  `ssl_check`, `url_verification`, signature, body limit and malformed input,
  are the gateway's, and the receiver does not see them. The receiver keeps its
  own ledger beside its tests, in the shape of the gateway's `ledger.json`,
  listing each such fixture id with the reason, so a fixture added later cannot
  be skipped silently.
- The settlement table in section 3 is tested with a listener that resolves,
  one that throws into an error handler that rethrows, and an invalid custom
  property, and the test asserts the resulting `ack`, `nak` or `term` and the
  redelivery count.

Bolt-level dispatch is then identical to `HTTPReceiver` and
`SocketModeReceiver` for every payload the fixtures cover, and a gateway change
to a header, a subject or a body shape fails both suites.

## Alternatives Rejected

- **Map `ack()` to the JetStream acknowledgement.** The most direct reading of
  Bolt's contract, and it would be correct if `ack` marked the end of the work.
  It does not: Bolt acknowledges events first, so a delivery would be settled
  before its listener starts, and a crash would lose it. Settling on
  `processEvent` costs a `working()` heartbeat and keeps the stream's
  guarantee.
- **Dispatch both the stored copy and the request.** Simpler to describe, and it
  runs a modal submission or a slash command twice. [ADR#0073](./0073-slack-acknowledgement-bodies-over-request-reply.md) accepts that a
  responder sees both and must be idempotent, which is workable for a purpose
  built responder and wrong for an existing Bolt listener written for exactly
  one call.
- **A per-app HTTP proxy that re-posts stored deliveries to the app's own
  `HTTPReceiver`.** Needs the signing secret again, to re-sign for Bolt's
  verification, so the secret is spread to every proxy. It makes every app an
  HTTP server for traffic that never touches the internet, and it holds the
  proxy's connection open for the acknowledgement body in the place the
  durable publish was meant to remove.
- **Fork Bolt.** A patched copy of `App` that reads NATS is a rebase on a
  library that releases often, to reach a seam Bolt already publishes as
  `Receiver`.
- **A non-Bolt SDK.** The right choice for a new application, and available. It
  does not serve the application that is being moved, whose listeners are the
  asset.
- **Verify signatures in the receiver.** Moves custody of the signing secret
  from one process to every consumer and repeats a check the gateway has
  already made.
- **An ephemeral consumer.** Position is lost on every restart, so an
  application that redeploys misses what the stream was holding for it.
- **`deliver_policy` `all` on creation.** Replays up to the stream's `max_age`
  into an application that has not seen it.

## Consequences

- An existing Bolt application moves behind the gateway by changing its
  receiver and supplying tokens through `authorize` or `token`. Its listeners,
  middleware and error handler are untouched.
- A stored delivery is at least once and a request is at most once. A listener
  on the stored path can see a `Nats-Msg-Id` twice. A listener on the request
  path runs once or not at all, as under Bolt's HTTP receiver.
- The window in which an application can be down and still catch up on stored
  deliveries is the stream's `max_age`, 7 days by default. A request has no such
  window.
- A reply has less time than Bolt's HTTP receiver gave a listener. The gateway
  publishes first, up to 1 second for a `view_submission` or a slash command,
  and the reply limit is 2.8 seconds from the start. A listener that
  acknowledged at 2.9 seconds over HTTP must now acknowledge sooner.
- The request path depends on the gateway sending a request. A Socket Mode
  envelope with `accepts_response_payload` false gets no request, and the stored
  `view_submission` it leaves is skipped by this receiver. Slack sets the flag
  on `view_submission` and on slash commands, so this is not expected to occur.
  If it does, the delivery is lost to the app, not redelivered.
- The gateway sends request bodies inline, so one larger than the NATS server's
  max payload cannot be sent and the delivery fails its acknowledgement. The
  stored copy is claim checked and unaffected.
- The receiver needs read access to the claim bucket in addition to the stream.
- A receiver reports `retryNum` as `undefined` for a first Socket Mode delivery
  where `SocketModeReceiver` reports `0`.
- The gateway in Rust and the receiver in TypeScript share no code, only the
  subjects, headers and fixtures. A change to any of them updates the gateway,
  the fixtures and the receiver together, and CI on either side catches a
  change made to one alone.
- Apps that depend on `customRoutes` or the OAuth install flow keep a separate
  HTTP process for them.

## References

- [ADR#0073: A Slack Acknowledgement Body Comes From a Consumer Over NATS Request/Reply, After the Durable Publish (Draft)](./0073-slack-acknowledgement-bodies-over-request-reply.md)
- [ADR#0044: Inbound Media Fetch Out of Band](./0044-inbound-media-fetch-out-of-band.md)
- [Slack Acknowledgement Responses](../architecture/slack-acknowledgement-responses.md)
- [bolt-js `types/receiver.ts`](https://github.com/slackapi/bolt-js/blob/3f8823c93f5639e99f05a2df9c7dc692da5cc184/src/types/receiver.ts)
- [bolt-js `App.ts` `processEvent`](https://github.com/slackapi/bolt-js/blob/3f8823c93f5639e99f05a2df9c7dc692da5cc184/src/App.ts)
- [bolt-js `SocketModeReceiver.ts`](https://github.com/slackapi/bolt-js/blob/3f8823c93f5639e99f05a2df9c7dc692da5cc184/src/receivers/SocketModeReceiver.ts)
- [bolt-js `HTTPResponseAck.ts`](https://github.com/slackapi/bolt-js/blob/3f8823c93f5639e99f05a2df9c7dc692da5cc184/src/receivers/HTTPResponseAck.ts)
- [bolt-js `HTTPModuleFunctions.ts`](https://github.com/slackapi/bolt-js/blob/3f8823c93f5639e99f05a2df9c7dc692da5cc184/src/receivers/HTTPModuleFunctions.ts)
- [Slack: using Socket Mode](https://docs.slack.dev/apis/events-api/using-socket-mode)

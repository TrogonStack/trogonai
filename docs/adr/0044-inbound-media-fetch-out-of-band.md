---
number: "0044"
slug: inbound-media-fetch-out-of-band
status: draft
date: 2026-08-02
---

# ADR#0044: Inbound Media Is Fetched Out of Band by a Dedicated Consumer

## Context

A chat platform does not deliver media inline. It delivers an opaque handle
(a Telegram `file_id`, a Slack `file.url_private`, a Discord attachment URL)
that only the holder of the platform credential can redeem. Something in this
system has to do that redemption, and where it happens determines which
processes need platform credentials, which components stop being generic, and
what a conversational turn has to wait for.

[The multi-channel routing design](../architecture/multi-channel-agent-routing.md)
originally recorded "eager claim-check": the bridge downloads media at
normalize time, before dispatching the prompt. That decision was never
implemented. `channel-bridge-telegram`'s parser drops every media update
(`parse.rs` returns `None` for any message without text and hardcodes
`attachments: Vec::new()`), so nothing is sunk and the decision is open.

Three properties of the problem constrain the answer:

1. **Redemption needs the platform credential.** Whoever fetches holds the bot
   token. This is the whole reason a handle cannot simply be passed downstream
   to a credential-free consumer.
2. **Redemption is slow and optional.** A turn that is pure text, or where the
   agent never opens the attached file, pays nothing for media it does not
   read. Fetching before dispatch makes every turn pay for the worst case.
3. **Handles are durable; redemption URLs are not.** A Telegram `file_id` is
   permanent and redeemable by the token holder indefinitely. What expires
   (roughly one hour) is the `file_path` that `getFile` returns. The original
   design rejected lazy fetch partly on "platform download URLs expire," which
   is true of the second step only and does not rule out deferring the first.

Three placements were considered.

**In the gateway.** `trogon-gateway` already holds a `TelegramBotToken` for
webhook registration, and the claim-check machinery with `ObjectStorePut` and
`ObjectStoreGet` already exists generically in `trogon-nats`. Both pieces are
in place, so the objection is not capability. The objection is scope: the
gateway is a verbatim transport shared across GitHub, GitLab, Linear, Slack,
Discord, and Telegram sources, and its stated contract is raw fidelity. Media
fetching would make one source materially smarter than its siblings, and would
put the bot token in the ingress. It also cannot be done in the request path at
all: Telegram retries webhooks that
do not return promptly, so a download inside the handler trades a fast ingress
for a slow one.

**In the bridge, before dispatch.** The original decision. It concentrates the
credential in a component that already has it and needs no coordination, but it
makes property 2 impossible: every turn waits for media the agent may never
read, on a path that is already fully serial.

**In a dedicated consumer.** JetStream already supports multiple independent
durables on one stream, so a second consumer of the raw stream costs no new
transport and no gateway change.

## Decision

### 1. The gateway stays a verbatim transport

`trogon-gateway` does not fetch media and does not gain per-source
intelligence. Its contract remains raw fidelity from webhook to stream. This is
a deliberate purchase: we accept a third credential holder (below) to keep the
ingress generic.

The gateway does already depend on an object store, through
`ClaimCheckPublisher`, which offloads any body over the NATS max payload and
publishes claim headers in its place. That is transport plumbing applied
identically to every source, not knowledge of what Telegram media is, so it does
not weaken this decision. It does mean any consumer of a raw stream must redeem
the claim before deserializing, the downloader below included. A consumer that
skips it gets no error, only an empty body.

### 2. A dedicated downloader consumes the raw stream on its own durable

A per-platform downloader (`channel-downloader-telegram` first) takes its own
durable consumer on the same raw stream the bridge reads, redeems the platform
handle with the bot token, and writes bytes through the existing
`ObjectStorePut` in `trogon-nats`. It is size-capped, and the cap is its own
configuration rather than the bridge's. For Telegram that cap is bounded from
above by the platform: the public Bot API refuses `getFile` for anything over
20 MB, so a larger configured cap only means something against a self-hosted Bot
API server, which is what lifts the limit.

Redemption is gated by the same identity check the bridge applies. The
downloader resolves the message's endpoint against `channel_endpoints_{prefix}`
and drops the update when that endpoint names no principal, before it calls
`getFile` and before anything reaches `ObjectStorePut`. Reading the raw stream
directly buys independence from the bridge, not exemption from its access
control. Without this check an unlinked chat could spend our bot token and our
object store by sending a file, and the bridge's later decision to drop the
message would arrive after the work was already done.

This is deliberately not a request/reply service. It is driven by the same
stream as the bridge, so a fetch begins at ingestion whether or not any agent
ever asks for the file, and the freshness window for `getFile` is entered
immediately rather than at some later moment of the agent's choosing.

### 3. Readiness is a KV status record, not an object-store lookup

An object store cannot answer "not yet." A `get` on a missing key returns
not-found, which is indistinguishable from a permanent failure and from a dead
downloader. Readiness therefore lives in its own JetStream KV bucket, keyed by
the credential that received the handle together with the handle itself:

```text
channel_media_{prefix}:
  {channel}.{account}.{platform_ref} ->
    { state: ready | failed, object_ref, mime, size, error }
```

The credential belongs in the key because a handle is not globally meaningful. A
Telegram `file_id` is issued per bot and redeemable only by the token that
received it, so the same string seen by two of our bot accounts denotes two
different files, and the bucket is channel-neutral besides. A bare handle key
would let one account's record answer for another's, and would point redemption
at a credential that cannot honor it. Readers already have both tokens: they are
the leading part of the endpoint on the event the attachment arrived with, and
they are what selects the token used to redeem.

Absence means not yet resolved, and it says nothing about whether work is under
way. The record is absent before the downloader's durable has reached the
message, while a download is running, and for as long as the downloader is down
or behind. A reader cannot tell those apart and does not need to. What it needs
is that absence is never permanent by accident, and two rules give it that.

The downloader writes a `failed` record for every permanent error, and also on
its last delivery attempt, which it recognizes from the delivery count JetStream
puts on the message. The write comes before the acknowledgement, and the
acknowledgement is what the write earns: a downloader that cannot reach the KV
bucket leaves the message unacknowledged, so JetStream redelivers and the
terminal record is attempted again on the next delivery. A handle whose
deliveries are exhausted therefore ends as a terminal record rather than as
silence, since a consumer that has stopped redelivering will never speak again on
its own.

Redelivery is a bounded number of attempts, so ordering the write before the ack
narrows the window in which a handle ends absent without closing it. A KV bucket
unreachable for the whole life of a message, or a downloader that dies between
its final read and its final write, exhausts the deliveries with nothing written.
That is the same shape as a downloader that never runs at all, and it has the
same backstop: the reader's deadline, not the terminal record, is what bounds how
long absence can last. The terminal record is what turns the failures a
downloader survives into an explanation the agent can be given instead of a
timeout, and it is not load-bearing for liveness.

Readers await readiness with a KV watch and a deadline, not a poll. A late
reader observes current state directly with no replay concern, and a deadline
expiry is reported to the agent as an unavailable attachment rather than as a
turn failure.

### 4. The inbound event carries the handle, never the object reference

`Attachment` on the inbound event carries `platform_ref` and drops
`object_ref`. The event states that a photo exists and gives its handle;
resolving that handle to bytes is a lookup performed later, not a field
populated earlier. An event that carried `object_ref` would be asserting the
presence of bytes that may not exist yet.

Outbound is not symmetric and does not change. `RenderCommand::SendAttachment`
keeps its `object_ref` because the agent produced that file and already put it
in the object store; there is no handle to redeem and nothing to wait for.

### 5. The turn does not block on media; the agent's tool does

The bridge builds the inbound event and dispatches the prompt without waiting.
Waiting happens inside the agent-facing download tool, at the moment the agent
actually opens the file. Text-only turns and turns that ignore an attachment
pay nothing.

## Invariants

- The gateway never interprets a source's payload beyond what publishing
  requires. Its object-store use is claim-check transport, identical for every
  source.
- No component blocks a conversational turn on media the agent has not asked
  for.
- Any component that redeems a platform handle holds that platform's
  credential; no credential-free component is ever handed a handle it is
  expected to resolve.
- A handle is redeemed with the credential of the account that received it, and
  is never keyed or cached in a way that lets one account's handle be resolved
  by another's.
- No handle is redeemed for an endpoint that resolves to no principal.
  Authorization precedes credential use, in every component that holds a
  credential.
- Readiness is always observable as an explicit state. "Bytes absent from the
  object store" is never interpreted as a lifecycle signal, and an absent
  readiness record is never read as an assertion that a download is running.
- Every handle a downloader stops working on leaves a terminal record, written
  before the message is acknowledged. Giving up is written down, not expressed by
  falling silent.
- No wait for readiness depends on a record arriving. A reader's deadline bounds
  absence on its own, so a downloader that cannot write its terminal record
  degrades the explanation an agent receives, never the reader's ability to stop
  waiting.
- An inbound event never asserts the existence of bytes that have not been
  written.

## Consequences

- **The bot token lives in three processes**: the gateway (webhook
  registration), the bridge (Bot API sends), and the downloader (`getFile`).
  This is the direct cost of keeping the gateway generic, and it is accepted.
  The evolution path already recorded in the routing design, a generic
  gateway **sink** concept that would centralize outbound token custody, is
  the eventual consolidation and remains out of scope here.
- **A third worker appears, but only when media does.** The routing design's
  "two workers total" claim holds until the first platform that carries media
  is supported. Nothing needs to be built before then.
- **A new KV bucket** (`channel_media_{prefix}`) joins the four the channel
  store already provisions.
- **Failure is legible.** A download that fails permanently is a `failed`
  record with a reason, distinguishable from one still in flight, so an agent
  can be told the difference. The cost is that the downloader has to write that
  record on the way out, including on its final delivery attempt and before it
  acknowledges, rather than letting the consumer's own give-up be the ending.
  Redelivery covers a write that fails while deliveries remain, and the reader's
  deadline covers the rest, which is why readers keep a deadline instead of
  trusting that a record always arrives.
- **The endpoints bucket gains a second reader.** `channel_endpoints_{prefix}`
  stays the bridge's to write, but the downloader reads it to authorize before
  redeeming, so identity is one registry consulted by every component that acts
  on a message rather than a check the bridge performs on everyone's behalf.
- **The downloader can be restarted or backfilled independently.** Because it
  is a durable consumer of a retained raw stream rather than a request/reply
  service, a downloader that was down comes back and works through what it
  missed without the bridge participating.
- **Two consumers now read the same raw stream.** This is ordinary JetStream
  usage, but it does mean the bridge no longer has exclusive knowledge of what
  arrived, and the two consumers' positions can differ.

## References

- [Multi-Channel Agent Routing](../architecture/multi-channel-agent-routing.md)
- [ADR#0024: Agent Platform Stream Topology](./0024-agent-platform-stream-topology.md)

# Multi-Channel Agent Routing

How a Telegram chat reaches an AI agent and gets a reply, and why the layer
between them knows nothing about Telegram. "Multi-channel" names the contract of
that middle layer, not a count of channels: Telegram is the only one
implemented, and everything between the platform edge and the agent is
channel-neutral so a second channel is a new edge, never a new brain.

## The shape in one paragraph

There is no per-channel intelligence. `trogon-gateway` owns the webhook and
publishes Telegram updates verbatim to NATS. `channel-bridge-telegram` consumes
them, normalizes each one into a channel-neutral event, resolves who is speaking
and which conversation they are in, and dispatches a prompt to an agent through
one in-process trait. Agents are plural and protocol-diverse (ACP today, A2A and
HTTP later) and are reached through adapters behind that trait, never through a
NATS namespace of their own. Replies stream back and render into the chat by
direct Bot API calls. All conversational state lives in JetStream KV, owned by
exactly one process.

## The path a message takes

```text
                 SUBJECT / PROTOCOL                      PROCESS

Telegram ─HTTP─▶ webhook validated, published verbatim   trogon-gateway
                 telegram.{update_type}, stream TELEGRAM (Telegram source, inbound only)
                      │
                      ▼ durable consumer, ack_wait 600s
                 normalize the Update into InboundEvent  channel-bridge-telegram
                 resolve principal + conversation in KV
                 dispatch the prompt through AgentPort
                      │
                      ▼ ACP over acp-nats
                 ═══ agent works, streams notifications ═══
                      │
                      ▼ buffer text, flush when the turn ends
                 Telegram Bot API ─HTTPS─▶               channel-bridge-telegram
                 send_message, chunked at 4096           (same process)
```

Two processes, and that is the whole topology. There is no subject between the
bridge and the agent other than the one `acp-nats` already owns, and no subject
between the bridge's halves, because it has no halves.

**The two legs are not symmetric.** Inbound goes through the gateway, which owns
the webhook and publishes verbatim. Outbound does not: the bridge holds a
`teloxide::Bot` and calls `send_message` and `send_chat_action` against the
Telegram API itself. The gateway has no outbound role, which is why the bot token
lives in both processes.

**Delivery is the consumer's configuration.** The durable is
`channel-bridge-telegram-{prefix}` on stream `TELEGRAM`
(`TELEGRAM_INBOUND_STREAM`). `DeliverPolicy::New` means a first run answers what
arrives from then on rather than replaying whatever the stream still retains;
restarts resume from the durable's own ack floor. `ack_wait` is 600 seconds
because a prompt turn legitimately runs for minutes, and a turn that fails is
left unacked so JetStream redelivers, bounded by `max_deliver` 5. The bridge
creates neither of the two resources it reads. The gateway provisions the stream
and the claim bucket, sizing the bucket's retention against the longest-retained
stream it serves; the bridge refuses to start if either resource is missing,
rather than create a wrong one and find out on the first oversized update. Which
bucket that is, `trogon-claims`, is a shared compile-time constant and not an
operator knob on either side: a claim only resolves in the bucket it was written
to, so any value the two could disagree on is a value one of them is wrong
about.

**The bridge handles one update at a time.** The inbound loop awaits each turn to
completion before pulling the next message. What the design requires is
per-conversation serialization; what runs is global serialization, which is
stricter than needed and a real limitation (see [Known gaps](#known-gaps)).

The guard that keeps this from being a monolith: everything channel-neutral (the
KV schemas and binding logic, the `AgentPort` trait, the inbound event and
render-command types) lives in the shared `trogon-channel` crate, not in the
Telegram binary. A second channel imports the same brain; it never copies it.

## Domain model

Four words carry this whole design, so here they are with values taken from the
running code. Each also has a one-paragraph entry in the
[glossary](../glossary/index.md) under "Channels and conversations".

An **endpoint** is a mailbox: one place messages arrive and leave. It is three
tokens, nothing more.

| Token | What it is | A real value |
| --- | --- | --- |
| `channel` | which platform | `telegram` |
| `account` | which of our bots on that platform | `mybot` |
| `peer` | which chat on the far side | `-1001234567890` (a group), `42` (a DM) |

Joined, that is `telegram.mybot.-1001234567890`, meaning "the chat
-1001234567890, talking to @mybot, on Telegram". That exact string is the KV key
(`Endpoint::kv_key`). Tokens are joined with `.`, so a token may not contain one,
and the permitted set narrows further to characters safe in both a NATS KV key
and a NATS subject token: that costs nothing and keeps the composite publishable
if it ever needs to be.

**`peer` is a chat, not a person.** Everyone in a group shares one endpoint,
which is why the sender is checked separately before a destructive command: the
chat being allowed says nothing about who spoke in it.

A **principal** is the identity an endpoint resolves to, normally a human. One
person can hold several endpoints (a Telegram DM, a Discord DM, the CLI) and they
all resolve to the same principal. That is the only thing that makes "continue
this conversation somewhere else" mean anything. A group chat is the exception,
covered below.

A **conversation** is the context the agent works in, and it is the root object:
endpoints point at it, never the reverse.

A **binding** is one KV entry, `endpoint -> conversation id`, and nothing more. A
message arrives, the bridge looks up its endpoint, and either finds a
conversation id or does not. If it does not, this is a new conversation: routing
policy picks the agent once, the conversation is created, and the entry is
written. The word makes it sound like a subsystem; it is a lookup table with one
column.

**Absence is judged per endpoint.** An endpoint with no entry starts a new
conversation even when its principal already holds one, because the lookup is
keyed by endpoint address and falls back to nothing.

`{prefix}`, which appears in bucket names below, is none of the above. It is the
deployment namespace (`CHANNEL_PREFIX`, default `prod`) so staging and production
can share a NATS cluster without sharing state. It never appears inside an
endpoint.

```text
endpoint (channel, account, peer)     where messages arrive and leave
   │                    │
   │ identity           │ binding: endpoint -> conversation id
   │ many-to-one        │ one entry per endpoint
   ▼                    │
principal               │             the human, across all channels
   │  owns              │
   ▼                    ▼
conversation                          the shared context
   ├── agent_id                       sticky: set at creation by routing policy
   └── current_session                ephemeral: belongs to the agent
```

Two arrows leave the endpoint because they are two separate lookups. Identity
answers "may this endpoint speak"; the binding answers "into which conversation".
A conversation records its principal, but nothing reads a conversation *by*
principal.

- **Conversations are cross-channel, by explicit link.** The conversation is the
  root object and endpoints are pointers into it, never the other way around, so
  several endpoints on different channels can point at one conversation and each
  will feed it. What the model does not do is infer that pointer: a shared
  principal is an access grant, not a route, so continuation from a second channel
  means writing that endpoint's binding to the existing conversation id. Reading
  the principal's most recent conversation instead would make continuation
  automatic and is deliberately not implemented, because guessing which
  conversation a new channel meant to resume is worse than starting a fresh one.
  Only Telegram implements an endpoint today, so none of this runs yet.
- **Binding is the session-routing record itself**, not a layer in front of it:
  an incoming message resolves endpoint to conversation and follows it. Routing
  policy (which agent handles a new conversation) is consulted exactly once, at
  conversation creation, then the binding is sticky. Operator config changes
  affect new conversations only; live conversations never silently change agents.
- **Sessions belong to agents and churn freely.** A session id alone is not
  routable (it is only meaningful at the agent that created it) and sessions die
  for boring reasons (reset, expiry, agent restart). Session replacement never
  re-runs routing policy and never changes the bound agent.
- Operations map onto the hierarchy: `/new` replaces `current_session` and keeps
  the agent; rebind is an explicit mutation of `agent_id` and discards the
  session; a stale session is repaired in place.
- **Reply-to-origin is per prompt, not per conversation.** Several endpoints can
  attach to one conversation, so each prompt records the endpoint it came from
  and the response renders there. Per-conversation serialization is mandatory:
  prompts from two endpoints into one session must queue in order.

### Groups: what an endpoint actually authorizes

Both lookups key on the **chat** endpoint, never on the sender's:
`principal_for(endpoint)` is the access gate and `conversation_for(endpoint)` is
the binding. The individual who typed is consulted in exactly one place,
`sender_is_authorized`, and only before a destructive command.

In a direct message the distinction is invisible, because Telegram gives a
private chat the same id as the user it belongs to. Seeding a user id through
`CHANNEL_SEED_TELEGRAM_USERS` writes the endpoint `telegram.{account}.{user_id}`,
which is also that DM's endpoint, so one write authorizes the person and their
first message creates the conversation bound to that same endpoint.

In a group the two come apart. A group's chat id is a distinct negative number
that equals no user id, so seeding members does nothing for the room: the bridge
finds no principal, logs, acks, and drops. A group works only once an operator
links the group's own chat endpoint to a principal, and then:

- **The room is the unit of conversation.** One endpoint means one binding, one
  conversation, one session. Members share the agent's context, which is the
  reason to put a bot in a room at all.
- **Authorizing the room authorizes everyone in it**, including whoever joins
  later. Ordinary messages have no per-member gate, deliberately. This is exactly
  why the narrower sender check guards `/new`: an unlinked member can talk to the
  agent but cannot destroy the room's session.
- **That principal names a room, not a human.** `ConversationRecord.principal`
  holds it, so for group conversations "principal" stops meaning "the person",
  and a room principal cannot participate in cross-channel continuation, which is
  a per-human idea.

The last point is a known imprecision, not a result we are happy with. The fix is
either a principal kind (human or room) or a conversation owner distinct from the
principal, and it stays deferred until a second channel makes cross-channel
continuation real. Until then groups work correctly and the vocabulary is
slightly dishonest about them.

## State: JetStream KV buckets

All stateful registries live in JetStream KV, owned exclusively by the bridge.
Config files carry only wiring (NATS connection, agent registry). The admin
surface for these buckets (CLI, config seeding, later GUI or MCP) is deliberately
out of scope; KV is the source of truth and whatever tool mutates it is
pluggable.

| Bucket | Key | Value |
| --- | --- | --- |
| `channel_principals_{prefix}` | principal id | display info, policy flags |
| `channel_endpoints_{prefix}` | endpoint address | principal id |
| `channel_bindings_{prefix}` | endpoint address | conversation id |
| `channel_conversations_{prefix}` | conversation id | principal id, agent_id, current_session, activity timestamps |

Access control is identity: an endpoint that resolves to no principal is rejected
at the bridge, which logs, acks, and drops. This replaces the per-channel
allowlist concept with one channel-neutral mechanism.

## Channel-neutral types

These live in `trogon-channel` and are the contract a second channel implements.
They are Rust types passed in process, and they are `Serialize` because the same
shapes are what any future transport between an edge and a router would carry.

**Inbound event**, what any channel bridge produces after stripping its
platform's shape:

```text
{
  endpoint:    { channel, account, peer },
  sender:      { platform_user_id, display_name },
  text:        string | null,
  command:     bridge command | null,
  attachments: [ { kind, mime, size, platform_ref } ],
  message_ref: platform message id (for dedup, replies, edits),
  occurred_at: timestamp
}
```

**Commands are extracted at the channel edge and never forwarded.** A trigger
(`/new`, `/reset`, configurable) counts only as the whole first token of a
message; anything after it stays in `text` and becomes the first prompt of
whatever the command sets up. Leading-slash vocabulary is a channel affordance, so
the bridge owns its own control words regardless of what the agent behind it
happens to advertise. A destructive command additionally authorizes the sender's
own endpoint rather than the conversation's, since a group chat is one endpoint
shared by everyone in it.

**Render commands**, the one output vocabulary every channel implements:

| Command | Purpose |
| --- | --- |
| `send_text` | new message |
| `edit_text` | streaming preview via edit-in-place |
| `send_attachment` | upload a produced file, by `object_ref` |
| `typing` | activity indicator |
| `react` | acknowledge without text |

It stays small on purpose. Both reference systems studied (OpenClaw, Hermes)
converged on essentially this set.

**Inbound media is fetched out of band** by a dedicated downloader on its own
durable consumer of the raw stream, never by the gateway and never inline in a
turn. The inbound event carries only `platform_ref`; readiness lives in a
`channel_media_{prefix}` KV record that a reader awaits by watch, at the moment
the agent opens the file. Outbound is not symmetric: `send_attachment` keeps its
`object_ref`, because the agent produced that file and there is nothing to
redeem. See [ADR#0044](../adr/0044-inbound-media-fetch-out-of-band.md). The
downloader is designed and not built; today media is dropped.

## Agent dispatch: the AgentPort trait

The bridge reaches agents through one in-process trait:

```text
AgentPort:
  create_session / resume_session
  prompt(session, content) -> stream of agent events
  cancel(session)
  release_session(session, reason) -> report
```

- `release_session` is infallible by signature. The conversation drops its
  pointer to the session and persists that *before* the agent is told, so a crash
  mid-reset orphans an agent session (recoverable) instead of resurrecting one the
  user asked to be rid of. Each step is capability-gated and best effort; an agent
  that cannot release must never wedge the conversation it was released from.
  Releasing is not deleting: the bridge is done with the session, which is not the
  same as the user asking for its history to be destroyed.
- Prompt failures rotate the session only when the agent says it does not have it.
  Timeouts and transport errors redeliver instead, because rotating on those
  discards a conversation that was merely unreachable for a moment.
- There is exactly one implementation, `AcpPort`, over the existing `acp-nats`
  client machinery, and it adopts only the session methods the agent advertises at
  initialize. A2A and HTTP are shapes the trait allows, not code that exists.
- The agent registry is config: `agent_id -> { protocol, address }` (for ACP: the
  acp prefix; the agent's workspace/cwd is agent configuration, never a channel
  concern).

## Carrying platform structure over ACP: the `_meta` convention

ACP reserves a `_meta` field on nearly every type (`PromptRequest`, every
`ContentBlock` variant, session notifications) explicitly for attaching arbitrary
metadata; `acp-nats` already uses it for prompt correlation. Three tiers of
Telegram structure map as follows:

1. **Content** (text, images, voice, documents): ACP content blocks directly. No
   loss. Claim-check references travel as embedded resources or links.
2. **Conversational context** (sender, reply-to, group vs DM, forwards): carried
   **twice, deliberately**. A human-readable prefix in the text block (works with
   any ACP agent, since only prompt text reaches the model) and a structured
   object in `PromptRequest._meta` (works richly with agents that opt in). `_meta`
   is machine-visible, not model-visible: a generic agent carries it and ignores
   it, which is safe.
3. **Platform interactivity** (inline buttons, callback queries, polls, edits):
   inbound, handled at the bridge and translated to synthetic prompt text ("user
   chose: Approve"). Outbound, an agent that participates in the convention
   attaches e.g. `{ telegram: { buttons: [...] } }` to a notification's `_meta`
   and the bridge renders it; event-shaped extensions use ACP `ExtNotification`.
   Agents that do not participate simply produce plain text, and the bot degrades
   gracefully.

Whatever the bridge does not carry is not destroyed: the raw `TELEGRAM` stream
retains full fidelity for replay when a future need appears. Fidelity there is
not the same thing as fidelity in the payload, though. The gateway publishes
through `ClaimCheckPublisher`, so an update larger than the NATS max payload is
stored in an object store and published as an empty body carrying claim headers.
The bytes are only visible to a consumer that redeems the claim, so the bridge
holds a `ClaimResolver` bound to the same bucket and resolves before it
deserializes. Any future consumer of a raw stream owes the same, and a consumer
that skips it does not see an error: it sees an empty body.

## Decisions and rejected alternatives

1. **No middle `channel.>` NATS namespace.** A topology where a per-platform
   "edge" publishes neutral events to
   `channel.{prefix}.in.{channel}.{account}.{peer}` for a channel-blind router to
   consume, and receives render commands back on a matching `out` subject, was
   considered and rejected. The prompt and notification traffic already crosses
   NATS inside `acp-nats`, so the extra namespace adds hops without adding a
   capability; raw-inbound replay is already covered by the `TELEGRAM` stream; and
   a neutral event that only ever travels in process needs no wire encoding, no
   subject scheme, and no direction token. What would reopen it: a second channel
   whose conversations can be continued from the first. Two ingress processes that
   each own identity, binding, and dispatch would both write the same KV buckets
   and could prompt one agent session concurrently, and the cheap fix for that is
   a single process owning conversations, which in turn needs the
   platform-specific processes to hand it neutral events over a stream. A second
   consumer of conversations (audit, analytics) reopens it for a different reason:
   an in-process event cannot be tapped.
2. **No `agents.>` NATS namespace; adapters are libraries.** Protocol-neutral
   agent addressability already exists twice in this workspace (`acp-nats` for
   ACP, `a2a-gateway` for A2A). A generic namespace would add a second hop and
   force redesigning streaming RPC over NATS, which `acp-nats` already solved.
   Revisit only if a service other than the bridge needs to prompt agents.
3. **Channel-neutral vocabulary lives in a shared crate**, not in the Telegram
   binary: `trogon-channel` owns the schemas, the KV logic, and `AgentPort`, and
   the Telegram crate owns only parsing and rendering.
4. **Conversation is the root; binding is sticky; policy runs once at creation.**
   Live conversations never hop agents because config changed.
5. **The chat is the unit of identity and conversation, not the speaker.** A group
   is one endpoint, so it gets one shared conversation and one shared
   authorization. Per-member conversations inside a room were rejected: they
   defeat the reason a bot is in a room. The cost is that a group's principal
   names a room rather than a person, accepted for now and revisited when
   cross-channel continuation becomes real.
6. **State in JetStream KV, not config files.** Admin surface out of band and
   unspecified (CLI/config now, GUI or MCP later).
7. **Inbound media fetched out of band** by a dedicated stream consumer, with
   readiness in KV and the wait deferred to the agent's own tool call
   ([ADR#0044](../adr/0044-inbound-media-fetch-out-of-band.md)).
8. **Platform structure over ACP via `_meta`**, dual-carried (text for any agent,
   `_meta` for participating agents); interactivity degrades gracefully with
   non-participating agents.
9. **Text rendering should stream via edit-in-place** (`edit_text`), the pattern
   both OpenClaw and Hermes converged on. Decided, not implemented: the Telegram
   `Outbound` trait has only `typing` and `send_text`, so today the bridge buffers
   and sends once at the end of the turn.
10. **Recorded here, not in ADRs**, except where a decision constrains a component
    outside this design. Media placement did, because it decides what the gateway
    is allowed to become, so it is
    [ADR#0044](../adr/0044-inbound-media-fetch-out-of-band.md).

## Known gaps

Things this design commits to that the running system does not do yet. None of
them change the topology above.

- **Inbound media is dropped.** Parsing keeps only the message text, so a photo,
  voice note, or document arrives as nothing at all.
  [ADR#0044](../adr/0044-inbound-media-fetch-out-of-band.md) settles where the
  fetch belongs; the downloader and the `channel_media_{prefix}` bucket do not
  exist.
- **No streaming output.** The renderer buffers agent text for the whole turn and
  sends it at the end, so the chat shows a typing indicator and then silence. Only
  `AgentMessageChunk` text is kept; tool calls, plans, thoughts, and non-text
  content blocks are logged and dropped, which is why an agent that answers only
  through tool output produces "Agent turn produced no text" and an empty chat.
- **Permission requests are always refused.** A chat has no permission surface, so
  `request_permission` returns `Cancelled`. That is the right default over
  silently granting, but it means the bridge only works against an agent
  configured not to ask.
- **A turn longer than `ack_wait` is prompted twice.** At 600 seconds the message
  becomes redeliverable while the turn is still running, and nothing dedups on
  `message_ref` even though the field exists for it. `max_deliver` 5 bounds the
  duplicates. The same path makes any redelivery after a partial turn re-prompt
  the agent.
- **Exhausted redeliveries go nowhere.** A message the bridge keeps failing on is
  dropped by JetStream after `max_deliver` 5 with no dead-letter subject, so a
  claim whose object genuinely expired, or a bucket misconfiguration, costs five
  loud failures and then silence. The failure is at least legible in the log,
  which is the difference from acking on the first attempt, but nothing holds the
  message for inspection.
- **The bridge exits if the agent is down at boot.** ACP `initialize` runs before
  the consumer opens and propagates its error, so an agent that is not yet
  reachable turns into a restart loop rather than a bridge that waits.
- **No per-conversation concurrency.** The inbound loop awaits each turn to
  completion, so one slow agent blocks every conversation, including the `/new`
  meant to rescue it. This is the largest operational gap.
- **Groups need a hand-written KV entry.** Only `CHANNEL_SEED_TELEGRAM_USERS`
  exists, and it seeds user-id endpoints, which are DM endpoints. A bot added to a
  room stays silent until someone links the room's own endpoint, and there is no
  admin surface for that.
- **A group's principal names a room, not a human**, so it cannot take part in
  cross-channel continuation.
- **Cross-channel continuation needs a binding written by hand**, and there is no
  admin surface that writes one. The model holds (the conversation is the root and
  takes pointers from any number of endpoints); what is missing is anything that
  creates the second pointer.
- **The bot token lives in two processes**, the gateway for webhook registration
  and the bridge for API calls. A generic gateway sink (NATS to HTTP-out,
  symmetric to its sources) would centralize outbound custody. It does not exist,
  and adding one is a gateway decision, not a channel one.
- **One agent protocol.** `AgentPort` has a single implementation.

## End-to-end walkthrough

1. A user sends "hello" to the bot on Telegram. Telegram POSTs the webhook;
   `trogon-gateway` validates it and publishes the raw Update to
   `telegram.message` on stream `TELEGRAM`.
2. `channel-bridge-telegram` consumes it on its durable, redeems the body if the
   message is a claim rather than a payload, parses the Update, and encodes the
   endpoint address.
3. The bridge resolves endpoint to principal, dropping the message if there is
   none; then endpoint to conversation, creating one via routing policy if absent
   and writing the sticky `agent_id`; then ensures a live session on that agent
   through the ACP adapter, creating or resuming.
4. The bridge dispatches the prompt with conversational context dual-carried (text
   prefix plus `_meta`), recording the origin endpoint for this prompt.
5. The agent streams session notifications back over `acp-nats`. The bridge sends
   one `typing` action before prompting, then accumulates the text of every
   `AgentMessageChunk` into a per-session buffer. Every other kind of session
   update is logged and dropped.
6. When the turn ends, the bridge takes the buffer and sends it with
   `send_message`, split at 4096 characters. Nothing reaches the chat before the
   turn is over, so a long turn shows a typing indicator and then silence.
7. The bridge acks the inbound message, which is when it becomes free to pull the
   next update.

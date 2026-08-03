# Multi-Channel Agent Routing

How a channel (Telegram first, Discord and others later) binds to an AI
agent. This document records two things: the **v1 implementation**, which goes
directly from the raw Telegram stream to an ACP agent through one bridge
worker, and the **multi-channel end state**, whose seams v1 keeps as code
boundaries so the extraction later is mechanical rather than a rewrite.

## The shape in one paragraph

There is no per-channel intelligence. A bridge translates between a platform
and the agent protocol; agents are plural and protocol-diverse (ACP today, A2A
and HTTP later) and are reached through in-process adapters behind a single
trait, never through a new NATS namespace. All conversational state (identity,
bindings, sessions) lives in JetStream KV, owned by exactly one worker, and is
designed channel-neutral from day one even while only Telegram exists.

## V1: the direct path

```
                 SUBJECTS / PROTOCOL                     WORKER

Telegram ─HTTP─▶ telegram.{update_type}                  trogon-gateway (exists)
                 stream TELEGRAM, raw verbatim JSON      (inbound path only)
                      │
                      ▼ durable consumer
                 normalize: parse Update, endpoint,      channel-bridge-telegram
                 identity + binding via KV,              (one worker, both halves)
                 dispatch prompt via AgentPort
                      │
                      ▼ acp-nats (already NATS-native)
                 ═══ agent works, streams notifications ═══
                      │
                      ▼ render notifications
                 Telegram Bot API calls ─HTTPS─▶         channel-bridge-telegram
                 (send, edit-in-place, chunk, throttle)  (same process as above)
```

**The two legs are not symmetric.** Inbound goes through the gateway, which
owns the webhook and publishes verbatim. Outbound does not: the bridge holds a
`teloxide::Bot` and calls `send_message`, `edit_message_text`, and
`send_chat_action` against the Telegram API itself. The gateway has no outbound
role in v1, which is why the bot token lives in both processes.

Two workers total, one of which already exists. The bridge is the fusion of
what the end state calls the "edge" and the "router". We fuse them because:

- The prompt/notification traffic **already crosses NATS** inside `acp-nats`;
  a `channel.>` middle namespace would add hops without adding a capability v1
  needs. ACP over NATS is our version of the direct function call OpenClaw and
  Hermes make in-process (both reference systems are monoliths whose channel
  handlers call the agent loop as a library).
- Raw-inbound replay is already covered by the gateway's `TELEGRAM` stream.
- The multi-channel benefits of the middle namespace only exist once there is
  a second channel or a second consumer.

The guard that keeps this from becoming a monolith: everything channel-neutral
(the KV schemas and binding logic, the `AgentPort` trait, the inbound event
and render-command types) lives in a **shared crate**, not in the Telegram
binary. A second channel imports the same brain; it never copies it.

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
today (`Endpoint::kv_key`) and becomes the tail of the NATS subject after
extraction. One value, two uses, no re-encoding, which is the reason the tokens
are restricted to characters that both KV keys and subject tokens accept.

**`peer` is a chat, not a person.** Everyone in a group shares one endpoint,
which is why the sender is checked separately before a destructive command: the
chat being allowed says nothing about who spoke in it.

A **principal** is the identity an endpoint resolves to, normally a human. One
person can hold several endpoints (a Telegram DM, a Discord DM, the CLI) and they
all resolve to the same principal. That is the only thing that makes "continue
this conversation somewhere else" mean anything. A group chat is the exception,
covered below.

A **conversation** is the context the agent works in, and it is the root
object: endpoints point at it, never the reverse.

A **binding** is one KV entry, `endpoint -> conversation id`, and nothing more.
A message arrives, the bridge looks up its endpoint, and either finds a
conversation id or does not. If it does not, this is a new conversation:
routing policy picks the agent once, the conversation is created, and the entry
is written. The word makes it sound like a subsystem; it is a lookup table with
one column.

`{prefix}`, which appears in bucket and stream names below, is none of the
above. It is the deployment namespace (`CHANNEL_PREFIX`, default `prod`) so
staging and production can share a NATS cluster without sharing state. It never
appears inside an endpoint.

```
endpoint (channel, account, peer)     where messages arrive and leave
   │  many-to-one
   ▼
principal                             the human, across all channels
   │
   ▼
conversation                          the shared context, cross-channel
   ├── agent_id                       sticky: set at creation by routing policy
   └── current_session                ephemeral: belongs to the agent
```

- **Conversations are cross-channel.** The same conversation can be picked up
  from Telegram, Discord, or the CLI. The conversation is the root object and
  endpoints are pointers into it, never the other way around.
- **Binding is the session-routing record itself**, not a layer in front of
  it: an incoming message resolves endpoint to conversation and follows it.
  Routing policy (which agent handles a new conversation) is consulted exactly
  once, at conversation creation, then the binding is sticky. Operator config
  changes affect new conversations only; live conversations never silently
  change agents.
- **Sessions belong to agents and churn freely.** A session id alone is not
  routable (it is only meaningful at the agent that created it) and sessions
  die for boring reasons (reset, expiry, agent restart). Session replacement
  never re-runs routing policy and never changes the bound agent.
- Operations map onto the hierarchy: `/new` replaces `current_session` and
  keeps the agent; rebind is an explicit mutation of `agent_id` and discards
  the session; a stale session is repaired in place.
- **Reply-to-origin is per prompt, not per conversation.** Several endpoints
  can attach to one conversation, so each prompt records the endpoint it came
  from and the response renders there. Per-conversation serialization is
  mandatory: prompts from two channels into one session queue in order.

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
that equals no user id, so seeding members does nothing for the room: the
bridge finds no principal, logs, acks, and drops. A group works only once an
operator links the group's own chat endpoint to a principal, and then:

- **The room is the unit of conversation.** One endpoint means one binding, one
  conversation, one session. Members share the agent's context, which is the
  reason to put a bot in a room at all.
- **Authorizing the room authorizes everyone in it**, including whoever joins
  later. Ordinary messages have no per-member gate, deliberately. This is
  exactly why the narrower sender check guards `/new`: an unlinked member can
  talk to the agent but cannot destroy the room's session.
- **That principal names a room, not a human.** `ConversationRecord.principal`
  holds it, so for group conversations "principal" stops meaning "the person",
  and a room principal cannot participate in cross-channel continuation, which
  is a per-human idea.

The last point is a known imprecision, not a result we are happy with. The fix
is either a principal kind (human or room) or a conversation owner distinct from
the principal, and it stays deferred until a second channel makes cross-channel
continuation real. Until then groups work correctly and the vocabulary is
slightly dishonest about them.

## The multi-channel end state

When a second channel or a second consumer of conversations (audit, analytics)
arrives, the bridge splits along the seams the shared crate already defines:

```
telegram.{update_type}  (stream TELEGRAM)                trogon-gateway
      │
      ▼
channel.{prefix}.in.{channel}.{account}.{peer}           channel-bridge-telegram
stream CHANNEL_IN_{prefix}, neutral inbound events       (normalize half)
      │
      ▼
[identity, binding, conversation KV,                     channel-router
 per-conversation serialization, AgentPort]              (generic, channel-blind)
      │
      ▼
channel.{prefix}.out.{channel}.{account}.{peer}          channel-router publishes,
stream CHANNEL_OUT_{prefix}, render commands             channel-bridge-telegram
      │                                                  (render half) consumes
      ▼
platform API calls
```

Filled in, an inbound subject reads
`channel.prod.in.telegram.mybot.-1001234567890`: the deployment, the direction,
then the endpoint address unchanged from its KV form.

- **`in` and `out` are relative to the user, not to the component.** The edge
  publishes to `in` and consumes from `out`; the router does the reverse, so the
  same token names one process's input and another's output. Naming the payload
  instead (`event` and `render`, matching the types the shared crate already
  defines) would remove the ambiguity. Left open deliberately: these subjects
  exist only on paper, and the choice belongs to the change that first creates
  them.
- The last three tokens are the **endpoint address** defined above. Edges own
  the encoding, which is why `Endpoint` refuses tokens that would not survive
  as a subject.
- Direction precedes the endpoint so the address stays a contiguous suffix,
  byte-identical to `Endpoint::kv_key()`. It also has to exist: the address is
  the same value both ways, so without it an inbound event and a render command
  for one chat would collide on one subject, the router would consume its own
  output, and `CHANNEL_IN_{prefix}` and `CHANNEL_OUT_{prefix}` could not be
  separate streams.
- The router subscribes `channel.{prefix}.in.>` and is channel-blind; a new
  channel is a new edge binary and zero router changes.
- The subjects carry exactly the types the shared crate already defines; the
  extraction is deployment surgery, not schema design.

## State: JetStream KV buckets

All stateful registries live in JetStream KV, owned exclusively by the bridge
(the router, after extraction). Config files carry only wiring (NATS
connection, agent registry). The admin surface for these buckets (CLI, config
seeding, later GUI or MCP) is deliberately out of scope; KV is the source of
truth and whatever tool mutates it is pluggable.

| Bucket | Key | Value |
| --- | --- | --- |
| `channel_principals_{prefix}` | principal id | display info, policy flags |
| `channel_endpoints_{prefix}` | endpoint address | principal id |
| `channel_bindings_{prefix}` | endpoint address | conversation id |
| `channel_conversations_{prefix}` | conversation id | principal id, agent_id, current_session, activity timestamps |

Access control is identity: an endpoint that resolves to no principal is
rejected (or ignored) at the bridge. This replaces the per-channel allowlist
concept with one channel-neutral mechanism.

## Shared-crate types (the wire schemas in waiting)

**Inbound event** (a Rust type in v1; the `channel.*.in.*` payload after
extraction):

```
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
whatever the command sets up. Leading-slash vocabulary is a channel affordance,
so the bridge owns its own control words regardless of what the agent behind it
happens to advertise. A destructive command additionally authorizes the sender's
own endpoint rather than the conversation's, since a group chat is one endpoint
shared by everyone in it.

**Render commands** (a Rust enum in v1; the `channel.*.out.*` payload after
extraction):

| Command | Purpose |
| --- | --- |
| `send_text` | new message |
| `edit_text` | streaming preview via edit-in-place |
| `send_attachment` | upload a produced file, by `object_ref` |
| `typing` | activity indicator |
| `react` | acknowledge without text |

The render vocabulary is the one contract every channel implements; it stays
small on purpose. Both reference systems studied (OpenClaw, Hermes) converged
on essentially this set.

**Inbound media is fetched out of band** by a dedicated downloader on its own
durable consumer of the raw stream, never by the gateway and never inline in a
turn. The inbound event carries only `platform_ref`; readiness lives in a
`channel_media_{prefix}` KV record that a reader awaits by watch, at the moment
the agent opens the file. Outbound is not symmetric: `send_attachment` keeps
its `object_ref`, because the agent produced that file and there is nothing to
redeem. See [ADR#0044](../adr/0044-inbound-media-fetch-out-of-band.md).

## Agent dispatch: the AgentPort trait

The bridge reaches agents through one in-process trait:

```
AgentPort:
  create_session / resume_session
  prompt(session, content) -> stream of agent events
  cancel(session)
  release_session(session, reason) -> report
```

- `release_session` is infallible by signature. The conversation drops its
  pointer to the session and persists that *before* the agent is told, so a
  crash mid-reset orphans an agent session (recoverable) instead of resurrecting
  one the user asked to be rid of. Each step is capability-gated and best
  effort; an agent that cannot release must never wedge the conversation it was
  released from. Releasing is not deleting: the bridge is done with the session,
  which is not the same as the user asking for its history to be destroyed.
- Prompt failures rotate the session only when the agent says it does not have
  it. Timeouts and transport errors redeliver instead, because rotating on those
  discards a conversation that was merely unreachable for a moment.

- v1 ships exactly one implementation: ACP, using the existing `acp-nats`
  client machinery. A2A and HTTP become additional implementations later.
- The agent registry is config: `agent_id -> { protocol, address }` (for ACP:
  the acp prefix; the agent's workspace/cwd is agent configuration, never a
  channel concern).

## Carrying platform structure over ACP: the `_meta` convention

ACP reserves a `_meta` field on nearly every type (`PromptRequest`, every
`ContentBlock` variant, session notifications) explicitly for attaching
arbitrary metadata; `acp-nats` already uses it for prompt correlation. Three
tiers of Telegram structure map as follows:

1. **Content** (text, images, voice, documents): ACP content blocks directly.
   No loss. Claim-check references travel as embedded resources or links.
2. **Conversational context** (sender, reply-to, group vs DM, forwards):
   carried **twice, deliberately**. A human-readable prefix in the text block
   (works with any ACP agent, since only prompt text reaches the model) and a
   structured object in `PromptRequest._meta` (works richly with agents that
   opt in). `_meta` is machine-visible, not model-visible: a generic agent
   carries it and ignores it, which is safe.
3. **Platform interactivity** (inline buttons, callback queries, polls,
   edits): inbound, handled at the bridge and translated to synthetic prompt
   text ("user chose: Approve"). Outbound, an agent that participates in the
   convention attaches e.g. `{ telegram: { buttons: [...] } }` to a
   notification's `_meta` and the bridge renders it; event-shaped extensions
   use ACP `ExtNotification`. Agents that do not participate simply produce
   plain text, and the bot degrades gracefully.

Whatever the bridge does not carry is not destroyed: the raw `TELEGRAM` stream
retains full fidelity for replay when a future need appears.

## Decisions and rejected alternatives

1. **V1 goes direct: one bridge worker, no `channel.>` subjects yet.** The
   neutral vocabulary ships as types in a shared crate; the namespace is the
   documented extraction path, triggered by a second channel or a second
   consumer. Rationale: acp-nats already provides the NATS seam and its
   buffering/observability; the middle namespace pays off only at channel two.
2. **Channel-neutral vocabulary from day one** even while fused: the shared
   crate, not the Telegram binary, owns the schemas, KV logic, and AgentPort.
   The `tgbot.>` subject space introduced during the Telegram refactor is
   transitional and gets absorbed.
3. **No `agents.>` NATS namespace; adapters are libraries.** Protocol-neutral
   agent addressability already exists twice in this workspace (`acp-nats`
   for ACP, `a2a-gateway` for A2A). A generic namespace would add a second
   hop and force redesigning streaming RPC over NATS, which `acp-nats`
   already solved. Revisit only if a service other than the bridge/router
   needs to prompt agents.
4. **Conversation is the root; binding is sticky; policy runs once at
   creation.** Live conversations never hop agents because config changed.
5. **The chat is the unit of identity and conversation, not the speaker.** A
   group is one endpoint, so it gets one shared conversation and one shared
   authorization. Per-member conversations inside a room were rejected: they
   defeat the reason a bot is in a room. The cost is that a group's principal
   names a room rather than a person, accepted for now and revisited when
   cross-channel continuation becomes real.
6. **State in JetStream KV, not config files.** Admin surface out of band and
   unspecified (CLI/config now, GUI or MCP later).
7. **Inbound media fetched out of band** by a dedicated stream consumer, with
   readiness in KV and the wait deferred to the agent's own tool call
   ([ADR#0044](../adr/0044-inbound-media-fetch-out-of-band.md)).
8. **Platform structure over ACP via `_meta`**, dual-carried (text for any
   agent, `_meta` for participating agents); interactivity degrades
   gracefully with non-participating agents.
9. **Text rendering via edit-in-place streaming** (`edit_text`), the pattern
   both OpenClaw and Hermes converged on.
10. **Recorded here, not in ADRs**, except where a decision constrains a
   component outside this design. Media placement did, because it decides what
   the gateway is allowed to become, so it is [ADR#0044](../adr/0044-inbound-media-fetch-out-of-band.md).

## Consequences for existing crates

- `telegram-agent`: its `llm.rs` and `conversation.rs` are the wrong layer
  (channels must not own a model loop) and disappear. Its consumer skeleton
  seeds `channel-bridge-telegram`.
- `telegram-bot`: its bridge/transform and outbound halves fold into
  `channel-bridge-telegram`, re-targeted at the shared-crate types; the typed
  Telegram event vocabulary in `telegram-types` is explicitly not the neutral
  model and shrinks to whatever the bridge still needs internally.
- `telegram-nats` (`tgbot.>` subjects, per-prefix streams): transitional,
  removed with the fusion (the bot-to-agent bus it modeled no longer exists
  as a NATS boundary in v1).
- `trogon-gateway`: unchanged. Its Telegram source stays the single raw
  ingress. Evolution path, not v1: a generic **sink** concept (NATS to
  HTTP-out) symmetric to its sources, which would centralize outbound token
  custody; today the bot token intentionally lives in both the gateway
  (webhook registration) and the bridge (API calls).

## End-to-end walkthrough (v1)

1. User sends "hello" to the bot on Telegram. Telegram POSTs the webhook;
   trogon-gateway validates and publishes the raw Update to
   `telegram.message` (stream `TELEGRAM`).
2. channel-bridge-telegram consumes it, parses the Update, and encodes the
   endpoint address. Any attachment contributes its `platform_ref` and nothing
   is downloaded on this path.
3. The bridge resolves endpoint to principal (reject if unknown), endpoint to
   conversation (create via routing policy if absent, writing the sticky
   `agent_id`), and ensures a live session on that agent through the ACP
   adapter (create or resume).
4. The bridge dispatches the prompt with conversational context dual-carried
   (text prefix + `_meta`), recording the origin endpoint for this prompt.
5. The agent streams session notifications over acp-nats. The bridge renders
   them: `typing`, then edit-in-place preview updates, finally the completed
   text, chunked at 4096 chars with edit throttling, plus any `_meta`-carried
   interactivity (buttons) the agent attached.
6. The same user later opens the CLI or Discord: a different endpoint mapped
   to the same principal binds to the same conversation and continues it;
   replies go to whichever endpoint prompted.

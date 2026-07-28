# Multi-Channel Agent Routing

How a chat channel (Telegram first, Discord and others later) binds to an AI
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
                 stream TELEGRAM, raw verbatim JSON
                      │
                      ▼ durable consumer
                 normalize: parse Update, endpoint,      chat-bridge-telegram
                 eager-download attachments              (one worker)
                 identity + binding via KV
                 dispatch prompt via AgentPort
                      │
                      ▼ acp-nats (already NATS-native)
                 ═══ agent works, streams notifications ═══
                      │
                      ▼ render notifications
                 Telegram Bot API calls                  chat-bridge-telegram
                 (send, edit-in-place, chunk, throttle)
```

Two workers total, one of which already exists. The bridge is the fusion of
what the end state calls the "edge" and the "router". We fuse them because:

- The prompt/notification traffic **already crosses NATS** inside `acp-nats`;
  a `chat.>` middle namespace would add hops without adding a capability v1
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

## The multi-channel end state

When a second channel or a second consumer of conversations (audit, analytics)
arrives, the bridge splits along the seams the shared crate already defines:

```
telegram.{update_type}  (stream TELEGRAM)                trogon-gateway
      │
      ▼
chat.{prefix}.in.{channel}.{account}.{peer}              chat-edge-telegram
stream CHAT_IN_{prefix}, neutral inbound events          (normalize half)
      │
      ▼
[identity, binding, conversation KV,                     chat-router
 per-conversation serialization, AgentPort]              (generic, channel-blind)
      │
      ▼
chat.{prefix}.out.{channel}.{account}.{peer}             chat-router publishes,
stream CHAT_OUT_{prefix}, render commands                chat-edge-telegram
      │                                                  (render half) consumes
      ▼
platform API calls
```

- `{channel}.{account}.{peer}` is the **endpoint address**; tokens must be
  subject-safe and edges own the encoding.
- The router subscribes `chat.{prefix}.in.>` and is channel-blind; a new
  channel is a new edge binary and zero router changes.
- The subjects carry exactly the types the shared crate already defines; the
  extraction is deployment surgery, not schema design.

## Domain model

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

## State: JetStream KV buckets

All stateful registries live in JetStream KV, owned exclusively by the bridge
(the router, after extraction). Config files carry only wiring (NATS
connection, agent registry). The admin surface for these buckets (CLI, config
seeding, later GUI or MCP) is deliberately out of scope; KV is the source of
truth and whatever tool mutates it is pluggable.

| Bucket | Key | Value |
| --- | --- | --- |
| `chat_principals_{prefix}` | principal id | display info, policy flags |
| `chat_endpoints_{prefix}` | endpoint address | principal id |
| `chat_bindings_{prefix}` | endpoint address | conversation id |
| `chat_conversations_{prefix}` | conversation id | principal id, agent_id, current_session, activity timestamps |

Access control is identity: an endpoint that resolves to no principal is
rejected (or ignored) at the bridge. This replaces the per-channel allowlist
concept with one channel-neutral mechanism.

## Shared-crate types (the wire schemas in waiting)

**Inbound chat event** (a Rust type in v1; the `chat.*.in.*` payload after
extraction):

```
{
  endpoint:    { channel, account, peer },
  sender:      { platform_user_id, display_name },
  text:        string | null,
  attachments: [ { kind, mime, size, object_ref, platform_ref } ],
  message_ref: platform message id (for dedup, replies, edits),
  occurred_at: timestamp
}
```

**Render commands** (a Rust enum in v1; the `chat.*.out.*` payload after
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

**Attachments are eager claim-check.** At normalize time the bridge downloads
the media from the platform (only the token holder can redeem a Telegram
`file_id`), stores the bytes in the object store, and the event carries the
reference. Nothing downstream ever needs platform credentials or a callback.
Lazy fetch-on-demand was rejected: it needs a request/reply surface, fails
mid-conversation instead of at ingestion, and platform download URLs expire.
Size is capped at the bridge.

## Agent dispatch: the AgentPort trait

The bridge reaches agents through one in-process trait:

```
AgentPort:
  create_session / resume_session
  prompt(session, content) -> stream of agent events
  cancel(session)
```

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

1. **V1 goes direct: one bridge worker, no `chat.>` subjects yet.** The
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
5. **State in JetStream KV, not config files.** Admin surface out of band and
   unspecified (CLI/config now, GUI or MCP later).
6. **Eager claim-check attachments**, size-capped at the bridge.
7. **Platform structure over ACP via `_meta`**, dual-carried (text for any
   agent, `_meta` for participating agents); interactivity degrades
   gracefully with non-participating agents.
8. **Text rendering via edit-in-place streaming** (`edit_text`), the pattern
   both OpenClaw and Hermes converged on.
9. **No ADRs for this**: local domain design, recorded here.

## Consequences for existing crates

- `telegram-agent`: its `llm.rs` and `conversation.rs` are the wrong layer
  (channels must not own a model loop) and disappear. Its consumer skeleton
  seeds `chat-bridge-telegram`.
- `telegram-bot`: its bridge/transform and outbound halves fold into
  `chat-bridge-telegram`, re-targeted at the shared-crate types; the typed
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
2. chat-bridge-telegram consumes it, parses the Update, encodes the endpoint
   address, and eager-downloads any attachments into the object store.
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

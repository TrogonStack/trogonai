# Channel Bridge Mechanics: how real systems connect channels to agents

Answers: how do OpenClaw, Hermes, Buzz, and community bridges handle instant
responsiveness, session mapping, and channel metadata; and is ACP-driven
channel bridging the professional pattern?

Verification status: the Buzz, telegram-acp-bot, and spec-extensibility
findings passed adversarial verification. The OpenClaw and Hermes sections
are single-pass but source-cited (Hermes largely from full source fetches;
OpenClaw from official docs plus the previously verified
[OpenClaw dossier](./products/openclaw.md)).

## The headline: two architectures exist, and the split is not what we assumed

**Architecture A: channel adapter as ACP client.** The adapter speaks ACP
directly to the agent. Used by the community bridges on the official clients
directory (telegram-acp-bot, Discord/Slack/WeChat/Lark/Matrix bridges) and by
Block's Buzz (buzz-acp harness).

**Architecture B: native channel gateway, ACP as a side door or backend.**
Channels ride a purpose-built native pipeline; ACP exists separately. Used by
BOTH production multi-channel systems studied:

- **OpenClaw**: WhatsApp/Telegram/Discord adapters do native Gateway session
  routing exclusively. ACP never carries channel traffic; it is an opt-in
  execution runtime (`runtime: "acp"`) that executes turns under an
  already-established channel session (hosting Codex, Claude Code, etc.).
- **Hermes**: Telegram/Discord/Slack/WhatsApp/Signal/WeChat/iMessage run
  through a separate `gateway` process with a typed presentation-event
  system. ACP is one of three front doors (ACP, TUI JSON-RPC, OpenAI HTTP),
  deliberately scoped to editors/IDEs. The one exception: buzz-acp drives
  `hermes acp` for the Buzz platform, documented as "a transport
  integration, not a second Hermes installation."

## Verdict for trogonai: ACP-driven, with the adapter playbook on top

Is ACP-driven the modern, professional solution? Split the question:

- **ACP as the agent boundary: yes, unambiguously.** Editors, external CLI
  hosting, and runtime interchange all converge on ACP (Goose is making it
  the primary interface for all its clients; OpenClaw's execution backends
  ride it; JetBrains co-governs it).
- **ACP as the channel adapter's wire: context-dependent.** Hermes and
  OpenClaw keep channels native because their agent cores predate ACP and
  their native event systems are richer than ACP's update vocabulary.
  trogonai's situation is different: its native client-to-agent boundary IS
  ACP over NATS by decision (ADRs 0003/0004/0011/0020). For trogonai,
  "native pipeline" and "ACP pipeline" are the same thing, so Architecture A
  over NATS is the coherent choice, and the OpenClaw/Hermes lesson is not
  "avoid ACP" but "ACP alone is not a channel UX; build the adapter
  playbook on top."

What ACP already gives the adapter: streamed `agent_message_chunk`,
`tool_call` / `tool_call_update` progress chrome, plan updates, turn-end
StopReason, `session/request_permission`. What it deliberately does not
cover, and what must live in the adapter regardless of architecture: acks
and typing indicators, streaming render modes and edit debouncing, message
chunking, queue semantics for mid-run messages, per-channel rendering
quirks, and channel metadata conventions.

## Responsiveness playbook (observed mechanics)

| Concern | OpenClaw | Hermes | telegram-acp-bot | Buzz (buzz-acp) |
|---|---|---|---|---|
| Instant ack | native reaction emoji (`messages.ackReactionScope`; WhatsApp 👀 + read receipts) | tool-progress bubbles as liveness | `ChatAction.TYPING` before the prompt call | none |
| Streaming | four-mode enum off/partial/block/progress; edit-in-place on Telegram/Discord; WhatsApp gets chunked sends (cannot edit) | `MessageChunk` events: native draft on Telegram DMs, edit-in-place elsewhere | three activity modes; compact mode self-animates ellipsis at 0.6s and edits one status message into the final reply | none; the agent posts via a `send_message` tool when it chooses |
| Edit debouncing | adapter buffers deltas, final on lifecycle end | dispatcher decides per channel | emit only on sentence boundary, min delta chars, min interval; 0.12s coalescing tick | n/a |
| Chunking | `textChunkLimit` 4000, newline-preferring; Discord native 2000 cap + 17-line breakpoint | per-platform | UTF-16-aware splitting via telegramify-markdown | n/a |

Cross-cutting invariants worth copying verbatim:

- Hermes: "Events describe transport, never context... History is owned by
  the agent; these events are a presentation-layer stream only." And:
  presentation failures must never raise into the agent's worker thread.
  Hermes refactored to typed events (`MessageChunk`, `MessageStop(final)`,
  `Commentary`, `ToolCallChunk`, `ToolCallFinished`) after tool bubbles and
  streaming drafts raced each other on Telegram under ad hoc callbacks.
- One channel-agnostic internal event model, rendered per platform by each
  adapter (OpenClaw and Hermes independently converged on this).

## Session mapping and concurrency

- **Session keys**: OpenClaw's scheme is explicit: DMs share
  `agent:main:main`; Discord guild channels get
  `agent:<agentId>:discord:channel:<channelId>`; Telegram forum topics
  append `:topic:<threadId>`; slash commands get isolated per-user sessions.
  Hermes routes on `agent:main:{platform}:{chat_type}[:{chat_id}][:{thread_id}][:{user_id}]`.
  telegram-acp-bot: one session per chat id, replaced wholesale on `/new`.
- **Mid-run second message**: OpenClaw names four queue modes, configurable
  globally, per channel, or per session (`/queue <mode>`): **steer** (inject
  after current tool batch), **followup** (one message, one later turn),
  **collect** (coalesce into one turn after a quiet window), **interrupt**
  (abort run, newest wins). telegram-acp-bot holds one queued slot plus a
  "Send now" button that fires session/cancel. Buzz drops or queues per
  mode, max one prompt in flight per channel, 50-event flush batches.
- **Serialization**: everyone serializes turns per session key (OpenClaw
  "session lanes"; Hermes `SessionTurnLeaseRegistry` with fail-open leases;
  buzz-acp in-flight flags). trogonai gets this naturally per NATS subject
  but must enforce single-flight per session in the adapter.
- **Group chats**: trigger on explicit mention; buffer unaddressed messages
  as context and inject with literal markers (OpenClaw WhatsApp: "[Chat
  messages since your last reply - for context]" / "[Current message -
  respond to this]", default buffer 50).
- **Reset**: explicit `/reset` and `/new` (OpenClaw allows `/new <model>`);
  background turns do not extend session freshness.

## Channel metadata: the spectrum (corrects the earlier absolute claim)

Earlier guidance said channel metadata never enters agent context.
Reality is a spectrum, and the professional systems DO inject it, carefully:

1. **Channel-blind** (telegram-acp-bot): session carries only cwd and
   mcpServers. Simplest, loses personalization and group awareness.
2. **Metadata as prompt text** (Buzz): each mention becomes an `[Event]`
   block with channel name/id, sender display name and pubkey, timestamp,
   raw tags, thread structure, preceded by `[Context]` sections. Plain
   text, no `_meta`.
3. **Untrusted-context envelope with echo-stripping** (OpenClaw Discord):
   channel name and member metadata appended to the prompt "as untrusted
   context... rather than visible reply prefixes," and if the model copies
   the envelope back, OpenClaw strips it from outbound replies and replay
   context. This is the most defensible pattern: metadata is available,
   explicitly marked untrusted, and hygiene-enforced at the boundary.
4. **Identity as construction parameters** (Hermes): `AIAgent` is built per
   session with `platform`, `user_id`, `thread_id`; per-channel
   `platform_toolsets` restrict tools; per-channel `tool_mode` and
   `preview_max_len` shape presentation.

Nobody uses ACP `_meta` for channel identity. The spec allows `_meta` on
every type (root keys reserved for W3C trace context; custom data must nest
under a namespaced key, e.g. goose's `_meta.goose.activeRunId`; custom
methods are underscore-prefixed like `_zed.dev/workspace/buffers`), but
defines no identity convention, so any channel-context-in-_meta scheme would
be trogonai-invented. Recommendation: adopt pattern 3 (untrusted-context
envelope with stripping) for prompt-visible metadata, and if structured
metadata must ride the wire for middleware, namespace it as
`_meta.trogonai.*` knowing it is non-standard.

## Approval UX ladder

OpenClaw escalates approval rendering to platform capability, all under one
"exec approval" concept: WhatsApp uses 👍/👎 reactions and numbered emoji
for choices; Telegram uses inline buttons (30-minute expiry); Discord uses
components v2 (up to 5 buttons or a select menu, per-button user ACLs,
modals, sensitive approvals defaulting to DM delivery because "approval
prompts include the command text"). For headless ACP-hosted runtimes it
falls back to policy: `permissionMode` (approve-all/approve-reads/deny-all)
x `nonInteractivePermissions` (fail/deny). Hermes' ACP permission bridge
maps allow_once/allow_session/allow_always/deny onto its own tiers, denying
by default on timeout. trogonai's session/request_permission handling should
implement exactly this ladder: channel-native interaction where the platform
can render it, policy engine where it cannot, deny-by-default on timeout.

## Also resolved: the Hermes session-persistence doc conflict

`acp_adapter/session.py` (fetched in full) confirms ACP sessions persist to
the shared `~/.hermes/state.db` and restore across restarts
(`SessionManager._restore()` reloads history and recreates the agent). The
user-guide page claiming process-scoped sessions is stale. Meta-lesson for
trogonai: keep ONE source of truth for ACP session lifecycle semantics; the
[Hermes dossier](./products/hermes-agent.md)'s verifier caught
this exact drift between two of Hermes' own doc pages.

## Design checklist for trogonai channel adapters

1. One channel-agnostic presentation-event stream (ACP session/update
   already provides most of the vocabulary); adapters render per platform.
2. Presentation is never persisted as history and never crashes the agent.
3. Ack immediately with the cheapest platform primitive (reaction, typing).
4. Streaming modes as named per-channel config (off/partial/block/progress),
   with quantitative edit debouncing and platform-aware chunking; no
   streaming where the platform cannot edit (WhatsApp pattern: chunked sends).
5. Queue modes as a named enum (steer/followup/collect/interrupt),
   overridable per channel and per session; single-flight per session key.
6. Session keys structured as platform:chat_type:chat_id:thread_id:user_id;
   explicit /new and /reset; background turns do not refresh idleness.
7. Group chats: mention-gated, with buffered context injected under literal
   markers.
8. Channel metadata via untrusted-context envelope with echo-stripping;
   never secrets (ADR 0032); `_meta.trogonai.*` only for structured
   middleware needs.
9. Approval ladder: platform-native UI where possible, policy engine for
   headless, deny on timeout.

## Sources

- https://docs.openclaw.ai/concepts/session, /concepts/agent-loop,
  /concepts/queue, /channels/telegram, /channels/whatsapp, /channels/discord
- https://github.com/NousResearch/hermes-agent: gateway/stream_events.py,
  gateway/stream_dispatch.py, gateway/turn_lease.py, gateway/platforms/,
  acp_adapter/session.py, website docs (acp.md, acp-internals.md,
  programmatic-integration.md)
- https://github.com/mgaitan/telegram-acp-bot (bridge.py, activity.py,
  acp/client.py, acp/service.py, core/session_registry.py)
- https://github.com/block/buzz: crates/buzz-acp (README, queue.rs, acp.rs,
  observer.rs)
- https://agentclientprotocol.com/protocol/extensibility
- https://agentclientprotocol.com/get-started/clients
- Prior verified dossiers: [OpenClaw](./products/openclaw.md),
  [Hermes](./products/hermes-agent.md),
  [Buzz](./products/buzz.md)

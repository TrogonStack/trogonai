# Channel Mapping: how a channel message reaches the agent

This page exists to remove one specific confusion: how channels (Telegram,
Slack, Discord, web console) relate to ACP and how a message gets to the
agent and back.

## The one rule (for trogonai)

**There is exactly one door into an agent: an ACP session.**

Everything that wants to talk to an agent is an ACP client. An editor is an
ACP client. A Telegram bridge is an ACP client. A Slack app is an ACP client.
A web console is an ACP client. They are all the same thing to the agent.

"Channel" is not an agent-side concept. It exists only inside the adapter.
The agent never hears the words Telegram, Slack, thread, or mention as
protocol concepts. By the time traffic reaches the agent, it is only ACP:
sessions, prompts, updates. (Channel metadata MAY still be injected as
prompt content; see the correction below.)

**Reality check (2026-07-30 follow-up research):** this rule is trogonai's
architecture, not a universal ecosystem fact. Community bridges and Buzz do
run channels as ACP clients (Architecture A), but both production
multi-channel systems studied keep the channel leg native: OpenClaw's
channel adapters use native Gateway session routing with ACP only as an
opt-in execution runtime underneath, and Hermes runs channels through a
separate gateway process with ACP scoped to editors. For trogonai the rule
still holds because its native client-to-agent boundary IS ACP over NATS by
decision, so "native pipeline" and "ACP pipeline" coincide. Full analysis
and the adapter playbook: [Channel Bridge Mechanics](./bridge-mechanics.md).

```
YOUR SURFACES                      THE ONE DOOR              THE AGENT
editor  ────────┐
Telegram bot ───┤   adapter per surface,   ┌──────────────┐
Slack app ──────┼── each one is an ACP ────┤  ACP session  ├──> agent process
web console ────┤   CLIENT                 └──────────────┘
A2A caller ─────┘
```

## End-to-end walkthrough (Telegram example)

What actually happens, step by step, when a user sends "fix the login bug"
in a Telegram group:

1. Telegram delivers the message to your bot via webhook. This is pure
   Telegram, no ACP yet.
2. The **channel adapter** (your code, lives in/behind trogon-gateway) looks
   up: does this chat/thread already have an ACP session?
   - No: adapter sends `session/new` to the agent (with cwd and mcpServers
     config) and stores the mapping `telegram thread 123 <-> sessionId abc`.
   - Yes: reuse the stored sessionId. (On wire v1 a restarted adapter can
     `session/load` to replay history; v2 draft changes this to
     `session/resume`.)
3. Adapter sends `session/prompt` with the message text as content blocks.
   One prompt in flight per session; queue further messages until the turn
   ends (this is exactly what buzz-acp does per channel).
4. The agent works. It streams `session/update` notifications back: message
   chunks, `tool_call`, `tool_call_update`, plan updates. The adapter
   translates each into channel form, e.g. edits a "typing..." Telegram
   message into growing text, or posts tool-call status lines.
5. **Mid-turn, the agent may call BACK into the adapter** (this is the part
   people miss). Three callback families, and the adapter must answer:
   - `session/request_permission` ("may I run this command?"): the adapter
     either renders it as channel-native UI (Telegram inline keyboard, Slack
     buttons) and waits for the human, or answers it from a policy engine
     for autonomous operation.
   - `fs/read_text_file`, `fs/write_text_file`: served against a sandboxed
     server-side workspace. There is no user laptop in this picture.
   - `terminal/*`: same, sandboxed workspace or not advertised at all.
   The adapter only advertises capabilities it can actually serve, at
   `initialize` time. If you advertise nothing, the agent must do everything
   through its own MCP servers instead.
6. The turn ends when the prompt response returns with a StopReason
   (`end_turn`, `refusal`, `cancelled`, ...). The adapter posts the final
   answer to the Telegram thread and releases the queue.

Replace "Telegram" with Slack, Discord, or the web console and NOTHING
changes except step 1 and the rendering in steps 4-5. That symmetry is the
entire point of putting channels behind ACP.

## What each side owns

| Concern | Who owns it |
|---|---|
| Threads, mentions, buttons, channel formatting | adapter only |
| conversation <-> sessionId mapping | adapter only |
| Prompt content, updates, StopReason | ACP wire |
| Permission decisions | adapter (human UI or policy engine) |
| Files and terminals | adapter, backed by sandboxed workspace |
| Model, tools, MCP servers, agent loop | agent |
| Remote delegation to other agents | agent, via A2A (never the channel path) |

## The trogonai wiring specifically

trogonai's transport is NATS, so the picture gets one extra hop, but roles
do not change:

```
Telegram/Slack/console
        v
channel adapter (ACP client role)          } trogon-gateway side
        v
ACP-over-NATS subjects (session.prompt,    } acp-nats crate family,
session.update, session.request_permission)  ADR 0011 jsonrpc-nats codec
        v
acp-host (to be built): NATS-side agent     } the missing component from
boundary that spawns and drives the real     the synthesis roadmap
agent as a stdio child
        v
gemini --acp | codex-acp | claude-agent-acp | goose acp | trogonai's own Rust agents
```

Notes anchored in the repo state at 058b8bee0:

- The subject vocabulary already exists (`session.new`, `session.prompt`,
  `session.update`, `session.request_permission`, `fs.*`, `terminal.*` in
  acp-nats `nats/parsing.rs`). The channel adapter publishes and consumes
  those subjects as the client peer.
- `session/request_permission` is currently a pure passthrough in acp-nats;
  the policy decision point must be added for autonomous channel operation.
- Secrets never ride in ACP messages (ADR 0032); provider keys are injected
  as env vars when acp-host spawns the agent (ADR 0023 custody design).
- Channel PROTOCOL semantics never enter ACP payloads (ADR 0003/0004
  layering), but channel CONTEXT may reach the agent as prompt content.
  Corrected 2026-07-30: Buzz bakes channel/sender/thread metadata into
  `[Event]` prompt blocks, and OpenClaw injects Discord channel/member
  metadata as an "untrusted context" envelope with echo-stripping. Nobody
  uses `_meta` for identity. Recommended trogonai pattern: untrusted-context
  envelope with stripping; secrets never (ADR 0032). Details:
  [Channel Bridge Mechanics](./bridge-mechanics.md).

## Ecosystem evidence

- Official ACP clients directory lists Discord, Slack, Telegram, WeChat, QQ,
  Lark, Matrix integrations as ACP clients; independent bridges (OpenACP,
  Sniptail, telegram-acp-bot) map forum topics/threads to sessions. See the
  [Zed case study](./products/zed.md) sources.
- [OpenClaw](./products/openclaw.md): channels in front,
  ACP-hosted agents behind a gateway, one session per conversation.
- [Buzz](./products/buzz.md): buzz-acp batches @mentions into
  session/prompt, one in flight per channel, sessions keyed per group.

Related: [Host Role and
Invocation Mechanics](./host-role-and-invocation.md) (who spawns the agent and how);
[ACP vs A2A](./acp-vs-a2a.md) (why A2A is not the channel
seat).
